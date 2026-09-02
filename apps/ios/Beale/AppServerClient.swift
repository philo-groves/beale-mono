import Foundation

enum AppServerClientError: LocalizedError, Equatable {
    case invalidEndpoint(String)
    case insecureEndpoint
    case invalidResponse
    case incompatible(String)
    case rejected(status: Int, message: String)

    var errorDescription: String? {
        switch self {
        case .invalidEndpoint(let value):
            return "\(value.isEmpty ? "A server URL" : value) is not a valid app-server endpoint."
        case .insecureEndpoint:
            return "Use an HTTPS Tailscale Serve URL. Plain HTTP is allowed only for localhost debug builds."
        case .invalidResponse:
            return "The app-server returned an invalid response."
        case .incompatible(let message):
            return message
        case .rejected(_, let message):
            return message
        }
    }
}

struct AppServerEndpoint: Equatable, Sendable {
    let baseURL: URL

    init(_ rawValue: String) throws {
        let trimmed = rawValue.trimmingCharacters(in: .whitespacesAndNewlines)
        guard var components = URLComponents(string: trimmed),
              let scheme = components.scheme?.lowercased(),
              let host = components.host,
              !host.isEmpty else {
            throw AppServerClientError.invalidEndpoint(trimmed)
        }

        let permitsHTTP: Bool
#if DEBUG
        permitsHTTP = host == "localhost" || host == "127.0.0.1" || host == "::1"
#else
        permitsHTTP = false
#endif
        guard scheme == "https" || (scheme == "http" && permitsHTTP) else {
            throw AppServerClientError.insecureEndpoint
        }
        guard components.user == nil,
              components.password == nil,
              components.query == nil,
              components.fragment == nil,
              components.path.isEmpty || components.path == "/" else {
            throw AppServerClientError.invalidEndpoint(trimmed)
        }

        components.scheme = scheme
        components.path = ""
        guard let normalized = components.url else {
            throw AppServerClientError.invalidEndpoint(trimmed)
        }
        baseURL = normalized
    }

    func url(path: String) -> URL {
        baseURL.appending(path: path.trimmingCharacters(in: CharacterSet(charactersIn: "/")))
    }

    func url(pathComponents: [String], queryItems: [URLQueryItem] = []) -> URL {
        let url = pathComponents.reduce(baseURL) { partialURL, component in
            partialURL.appending(path: component)
        }
        guard !queryItems.isEmpty,
              var components = URLComponents(url: url, resolvingAgainstBaseURL: false) else {
            return url
        }
        components.queryItems = queryItems
        return components.url ?? url
    }

    func webSocketURL(path: String) throws -> URL {
        guard var components = URLComponents(url: url(path: path), resolvingAgainstBaseURL: false) else {
            throw AppServerClientError.invalidResponse
        }
        components.scheme = components.scheme == "https" ? "wss" : "ws"
        guard let result = components.url else {
            throw AppServerClientError.invalidResponse
        }
        return result
    }
}

struct AppServerSnapshot: Sendable {
    let health: AppServerHealth
    let providerCatalog: AppServerProviderCatalog
    let workspaces: [AppServerWorkspace]
    let sessions: [AppServerSession]
}

struct AppServerClient: Sendable {
    let endpoint: AppServerEndpoint
    let operatorToken: String
    private let session: URLSession
    private let decoder = JSONDecoder()

    init(endpoint: AppServerEndpoint, operatorToken: String, session: URLSession = .shared) {
        self.endpoint = endpoint
        self.operatorToken = operatorToken
        self.session = session
    }

    func fetchSnapshot() async throws -> AppServerSnapshot {
        let health: AppServerHealth = try await get(path: "/health", authenticated: false)
        try health.validateCompatibility()

        let descriptor: AppServerDescriptor = try await get(path: "/v1/server", authenticated: true)
        try descriptor.validateCompatibility()

        async let providerCatalog: AppServerProviderCatalog = get(path: "/v1/providers", authenticated: true)
        async let workspaceList: AppServerWorkspaceList = get(path: "/v1/workspaces", authenticated: true)
        async let sessionCatalog: AppServerSessionCatalog = get(path: "/v1/sessions", authenticated: true)
        let (providers, workspaces, sessions) = try await (providerCatalog, workspaceList, sessionCatalog)

        try providers.validateCompatibility()
        guard workspaces.controlVersion == BealeAppServerContract.controlVersion,
              sessions.controlVersion == BealeAppServerContract.controlVersion else {
            throw AppServerClientError.incompatible("An app-server response used an unsupported control version.")
        }
        return AppServerSnapshot(
            health: health,
            providerCatalog: providers,
            workspaces: workspaces.workspaces,
            sessions: sessions.sessions
        )
    }

    func fetchWorkspaceSessions(workspaceId: String, limit: Int = 200) async throws -> [AppServerWorkspaceSession] {
        let boundedLimit = min(max(limit, 1), 200)
        let url = endpoint.url(
            pathComponents: ["v1", "workspaces", workspaceId, "sessions"],
            queryItems: [URLQueryItem(name: "limit", value: String(boundedLimit))]
        )
        let response: AppServerCanonicalWorkspaceSessions = try await get(url: url, authenticated: true)
        guard response.controlVersion == BealeAppServerContract.controlVersion,
              response.workspace.workspaceId == workspaceId else {
            throw AppServerClientError.incompatible("The workspace session response did not match this request.")
        }
        return response.result.sorted { $0.updatedAt > $1.updatedAt }
    }

    func fetchMemoryNotifications(
        workspaceId: String,
        sessionId: String? = nil
    ) async throws -> AppServerMemoryNotificationFeed {
        let queryItems = sessionId.map { [URLQueryItem(name: "sessionId", value: $0)] } ?? []
        let url = endpoint.url(
            pathComponents: ["v1", "workspaces", workspaceId, "memory-notifications"],
            queryItems: queryItems
        )
        let response: AppServerCanonicalMemoryNotifications = try await get(url: url, authenticated: true)
        return try response.validatedResult(workspaceId: workspaceId)
    }

    func fetchWorkspaceMemory(workspaceId: String) async throws -> AppServerWorkspaceMemoryCatalog {
        let url = endpoint.url(pathComponents: ["v1", "workspaces", workspaceId, "memory"])
        let response: AppServerCanonicalWorkspaceMemory = try await get(url: url, authenticated: true)
        return try response.validatedResult(workspaceId: workspaceId)
    }

    func fetchSessionTranscript(
        workspaceId: String,
        sessionId: String,
        afterEventId: String? = nil,
        tail: Bool = false,
        limit: Int = 200
    ) async throws -> AppServerSessionEventPage {
        let boundedLimit = min(max(limit, 1), 200)
        var queryItems = [
            URLQueryItem(name: "stream", value: "commentary"),
            URLQueryItem(name: "limit", value: String(boundedLimit)),
            URLQueryItem(name: "maxBytes", value: "1000000")
        ]
        if let afterEventId, !afterEventId.isEmpty {
            queryItems.append(URLQueryItem(name: "afterEventId", value: afterEventId))
        }
        if tail {
            queryItems.append(URLQueryItem(name: "tail", value: "true"))
        }

        let url = endpoint.url(
            pathComponents: ["v1", "workspaces", workspaceId, "sessions", sessionId, "events"],
            queryItems: queryItems
        )
        let response: AppServerCanonicalSessionEvents = try await get(url: url, authenticated: true)
        guard response.controlVersion == BealeAppServerContract.controlVersion,
              response.workspace.workspaceId == workspaceId,
              response.result.sessionId == sessionId,
              response.result.stream == "commentary" else {
            throw AppServerClientError.incompatible("The session commentary response did not match this request.")
        }
        return response.result
    }

    func fetchSessionCollaboration(
        workspaceId: String,
        sessionId: String,
        messageLimit: Int = 200
    ) async throws -> AppServerSessionCollaborationState {
        let boundedLimit = min(max(messageLimit, 1), 1_000)
        let url = endpoint.url(
            pathComponents: ["v1", "workspaces", workspaceId, "sessions", sessionId, "collaboration"],
            queryItems: [URLQueryItem(name: "messageLimit", value: String(boundedLimit))]
        )
        let response: AppServerCanonicalSessionCollaboration = try await get(url: url, authenticated: true)
        guard response.controlVersion == BealeAppServerContract.controlVersion,
              response.workspace.workspaceId == workspaceId,
              response.result.sessionId == sessionId else {
            throw AppServerClientError.incompatible("The session collaboration response did not match this request.")
        }
        return response.result
    }

    func fetchSessionApprovalEvents(
        workspaceId: String,
        sessionId: String,
        limit: Int = 500
    ) async throws -> AppServerSessionApprovalEventPage {
        let boundedLimit = min(max(limit, 1), 500)
        let url = endpoint.url(
            pathComponents: ["v1", "workspaces", workspaceId, "sessions", sessionId, "events"],
            queryItems: [
                URLQueryItem(name: "stream", value: "trace"),
                URLQueryItem(name: "limit", value: String(boundedLimit)),
                URLQueryItem(name: "maxBytes", value: "1000000"),
                URLQueryItem(name: "tail", value: "true")
            ]
        )
        let response: AppServerCanonicalSessionApprovalEvents = try await get(url: url, authenticated: true)
        guard response.controlVersion == BealeAppServerContract.controlVersion,
              response.workspace.workspaceId == workspaceId,
              response.result.sessionId == sessionId,
              response.result.stream == "trace" else {
            throw AppServerClientError.incompatible("The session approval response did not match this request.")
        }
        return response.result
    }

    func startSession(
        workspace: AppServerWorkspace,
        promptMarkdown: String,
        configuration: AppServerSessionLaunchConfiguration
    ) async throws -> AppServerSessionStart {
        let body = try JSONEncoder().encode(
            AppServerSessionLaunchRequest(
                workspace: workspace,
                promptMarkdown: promptMarkdown,
                configuration: configuration
            )
        )
        let response: AppServerSessionStart = try await request(
            url: endpoint.url(path: "/v1/sessions"),
            method: "POST",
            authenticated: true,
            body: body,
            timeoutInterval: 35
        )
        try response.validateCompatibility()
        return response
    }

    func stopSession(sessionId: String) async throws -> Bool {
        do {
            let response: AppServerSessionStopResult = try await request(
                url: endpoint.url(pathComponents: ["v1", "sessions", sessionId]),
                method: "DELETE",
                authenticated: true
            )
            try response.validate(sessionId: sessionId)
            return response.stopped
        } catch AppServerClientError.rejected(let status, _) where status == 404 {
            // The worker may finish between the canonical status poll and the stop request.
            return false
        }
    }

    func generateResearchSuggestions(
        workspaceId: String,
        refresh: Bool = false
    ) async throws -> AppServerGeneratedResearchSuggestions {
        let response: AppServerOperationResponse<AppServerGeneratedResearchSuggestions> = try await operation(
            "suggestion.generate",
            input: AppServerResearchSuggestionOperationInput(
                workspaceId: workspaceId,
                refresh: refresh
            ),
            timeoutInterval: 35
        )
        guard response.controlVersion == BealeAppServerContract.controlVersion else {
            throw AppServerClientError.incompatible("The research suggestions used an unsupported control version.")
        }
        return response.result
    }

    func expandResearchPrompt(
        workspaceId: String,
        promptMarkdown: String,
        phase: String?,
        configuration: AppServerSessionLaunchConfiguration
    ) async throws -> AppServerExpandedResearchPrompt {
        let response: AppServerOperationResponse<AppServerExpandedResearchPrompt> = try await operation(
            "prompt.expand",
            input: AppServerResearchPromptExpansionOperationInput(
                workspaceId: workspaceId,
                promptMarkdown: promptMarkdown,
                phase: phase,
                configuration: configuration
            ),
            timeoutInterval: 60
        )
        guard response.controlVersion == BealeAppServerContract.controlVersion else {
            throw AppServerClientError.incompatible("The expanded research request used an unsupported control version.")
        }
        return response.result
    }

    func createSessionAttachment(sessionId: String) async throws -> AppServerSessionAttachment {
        let url = endpoint.url(pathComponents: ["v1", "sessions", sessionId, "attachments"])
        let attachment: AppServerSessionAttachment = try await request(
            url: url,
            method: "POST",
            authenticated: true
        )
        guard attachment.controlVersion == BealeAppServerContract.controlVersion,
              attachment.session.sessionId == sessionId,
              attachment.session.isActive,
              attachment.transport.protocolVersion == BealeAppServerContract.appServerProtocolVersion,
              attachment.transport.authentication == "bearer",
              attachment.transport.reconnect == "replay",
              !attachment.transport.token.isEmpty else {
            throw AppServerClientError.incompatible("The session attachment did not match this request.")
        }
        return attachment
    }

    private func get<Response: Decodable & Sendable>(
        path: String,
        authenticated: Bool
    ) async throws -> Response {
        try await get(url: endpoint.url(path: path), authenticated: authenticated)
    }

    private func get<Response: Decodable & Sendable>(
        url: URL,
        authenticated: Bool
    ) async throws -> Response {
        try await request(url: url, method: "GET", authenticated: authenticated)
    }

    private func operation<Input: Encodable & Sendable, Result: Decodable & Sendable>(
        _ operation: String,
        input: Input,
        timeoutInterval: TimeInterval
    ) async throws -> AppServerOperationResponse<Result> {
        let body = try JSONEncoder().encode(
            AppServerOperationRequest(operation: operation, input: input)
        )
        return try await request(
            url: endpoint.url(path: "/v1/operations"),
            method: "POST",
            authenticated: true,
            body: body,
            timeoutInterval: timeoutInterval
        )
    }

    private func request<Response: Decodable & Sendable>(
        url: URL,
        method: String,
        authenticated: Bool,
        body: Data? = nil,
        timeoutInterval: TimeInterval = 15
    ) async throws -> Response {
        var request = URLRequest(url: url)
        request.httpMethod = method
        request.timeoutInterval = timeoutInterval
        request.setValue("application/json", forHTTPHeaderField: "Accept")
        if let body {
            request.httpBody = body
            request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        }
        if authenticated {
            request.setValue("Bearer \(operatorToken)", forHTTPHeaderField: "Authorization")
        }

        let (data, response) = try await session.data(for: request)
        guard let httpResponse = response as? HTTPURLResponse else {
            throw AppServerClientError.invalidResponse
        }
        guard (200..<300).contains(httpResponse.statusCode) else {
            let serverError = try? decoder.decode(AppServerErrorResponse.self, from: data)
            throw AppServerClientError.rejected(
                status: httpResponse.statusCode,
                message: serverError?.error.message ?? "The app-server rejected the request (\(httpResponse.statusCode))."
            )
        }
        do {
            return try decoder.decode(Response.self, from: data)
        } catch {
            throw AppServerClientError.invalidResponse
        }
    }
}

@MainActor
final class AppServerSessionControlChannel {
    private struct ServerEnvelope: Decodable {
        struct Event: Decodable {
            struct Payload: Decodable {
                let eventType: String?
                let requestId: String?
                let accepted: Bool?
                let error: String?
            }

            let kind: String
            let payload: Payload?
        }

        struct ProtocolError: Decodable {
            let message: String
        }

        let protocolVersion: Int
        let type: String
        let sessionId: String
        let requestId: String?
        let event: Event?
        let error: ProtocolError?
    }

    private struct PendingAcknowledgement {
        let continuation: CheckedContinuation<Void, Error>
        let timeout: Task<Void, Never>
    }

    private let endpoint: AppServerEndpoint
    private let sessionId: String
    private let transport: AppServerSessionTransport
    private let urlSession: URLSession
    private var socket: URLSessionWebSocketTask?
    private var receiveTask: Task<Void, Never>?
    private var pendingAcknowledgements: [String: PendingAcknowledgement] = [:]
    private var earlyAcknowledgements: [String: Result<Void, Error>] = [:]
    private(set) var isConnected = false

    init(
        endpoint: AppServerEndpoint,
        sessionId: String,
        transport: AppServerSessionTransport,
        urlSession: URLSession = .shared
    ) {
        self.endpoint = endpoint
        self.sessionId = sessionId
        self.transport = transport
        self.urlSession = urlSession
    }

    func connect() async throws {
        guard !isConnected else { return }
        var request = URLRequest(url: try endpoint.webSocketURL(path: transport.path))
        request.timeoutInterval = 15
        request.setValue("Bearer \(transport.token)", forHTTPHeaderField: "Authorization")
        let socket = urlSession.webSocketTask(with: request)
        self.socket = socket
        socket.resume()
        try await socket.send(.string(try jsonString([
            "protocolVersion": BealeAppServerContract.appServerProtocolVersion,
            "type": "client.hello",
            "sessionId": sessionId,
            "client": ["name": "beale-ios", "version": "0.1.0"]
        ])))

        let hello = try await socket.receive()
        let envelope = try decodeEnvelope(hello)
        guard envelope.protocolVersion == BealeAppServerContract.appServerProtocolVersion,
              envelope.sessionId == sessionId,
              envelope.type == "server.hello" else {
            close()
            throw AppServerClientError.incompatible("The session transport returned an invalid handshake.")
        }
        isConnected = true
        receiveTask = Task { [weak self] in
            await self?.receiveMessages()
        }
    }

    func sendSteering(_ instruction: String) async throws -> String {
        try await sendControl(type: "steer", fields: ["instruction": instruction])
    }

    func sendApproval(
        approvalRequestId: String,
        requestKind: String,
        decision: String
    ) async throws -> String {
        guard decision == "approved" || decision == "denied" else {
            throw AppServerClientError.rejected(status: 400, message: "The approval decision is invalid.")
        }
        let controlType = requestKind == "computer_use" ? "resolve_tool_approval" : "resolve_shell_approval"
        return try await sendControl(type: controlType, fields: [
            "approvalRequestId": approvalRequestId,
            "decision": decision
        ])
    }

    private func sendControl(type: String, fields: [String: String]) async throws -> String {
        guard isConnected, let socket else {
            throw AppServerClientError.rejected(status: 503, message: "The live session connection is unavailable.")
        }
        let requestId = "ios_\(UUID().uuidString.lowercased())"
        var control: [String: Any] = [
            "schemaVersion": 1,
            "type": type,
            "requestId": requestId
        ]
        for (key, value) in fields {
            control[key] = value
        }
        try await socket.send(.string(try jsonString([
            "protocolVersion": BealeAppServerContract.appServerProtocolVersion,
            "type": "session.control",
            "sessionId": sessionId,
            "requestId": requestId,
            "control": control
        ])))
        try await waitForAcknowledgement(requestId: requestId)
        return requestId
    }

    func close() {
        isConnected = false
        let pending = pendingAcknowledgements
        pendingAcknowledgements = [:]
        earlyAcknowledgements = [:]
        for acknowledgement in pending.values {
            acknowledgement.timeout.cancel()
            acknowledgement.continuation.resume(
                throwing: AppServerClientError.rejected(
                    status: 503,
                    message: "The live session connection closed before app-server acknowledged the message."
                )
            )
        }
        receiveTask?.cancel()
        receiveTask = nil
        socket?.cancel(with: .normalClosure, reason: nil)
        socket = nil
    }

    private func receiveMessages() async {
        guard let socket else { return }
        while !Task.isCancelled {
            do {
                let message = try await socket.receive()
                let envelope = try decodeEnvelope(message)
                guard envelope.protocolVersion == BealeAppServerContract.appServerProtocolVersion,
                      envelope.sessionId == sessionId,
                      envelope.type != "protocol.error" else {
                    if let requestId = envelope.requestId {
                        resolveAcknowledgement(
                            requestId: requestId,
                            result: .failure(AppServerClientError.rejected(
                                status: 400,
                                message: envelope.error?.message ?? "app-server rejected the session message."
                            ))
                        )
                    }
                    close()
                    return
                }
                if envelope.type == "session.event",
                   envelope.event?.kind == "agent.event",
                   envelope.event?.payload?.eventType == "control.received",
                   let requestId = envelope.event?.payload?.requestId,
                   requestId.hasPrefix("ios_") {
                    let accepted = envelope.event?.payload?.accepted == true
                    resolveAcknowledgement(
                        requestId: requestId,
                        result: accepted
                            ? .success(())
                            : .failure(AppServerClientError.rejected(
                                status: 409,
                                message: envelope.event?.payload?.error ?? "app-server rejected the message."
                            ))
                    )
                }
            } catch {
                close()
                return
            }
        }
    }

    private func waitForAcknowledgement(requestId: String) async throws {
        if let early = earlyAcknowledgements.removeValue(forKey: requestId) {
            return try early.get()
        }
        try await withCheckedThrowingContinuation { continuation in
            let timeout = Task { [weak self] in
                try? await Task.sleep(for: .seconds(15))
                guard !Task.isCancelled else { return }
                await self?.resolveAcknowledgement(
                    requestId: requestId,
                    result: .failure(AppServerClientError.rejected(
                        status: 504,
                        message: "app-server did not acknowledge the message in time."
                    ))
                )
            }
            pendingAcknowledgements[requestId] = PendingAcknowledgement(
                continuation: continuation,
                timeout: timeout
            )
        }
    }

    private func resolveAcknowledgement(requestId: String, result: Result<Void, Error>) {
        if let pending = pendingAcknowledgements.removeValue(forKey: requestId) {
            pending.timeout.cancel()
            pending.continuation.resume(with: result)
            return
        }
        earlyAcknowledgements[requestId] = result
        if earlyAcknowledgements.count > 32, let oldest = earlyAcknowledgements.keys.first {
            earlyAcknowledgements.removeValue(forKey: oldest)
        }
    }

    private func decodeEnvelope(_ message: URLSessionWebSocketTask.Message) throws -> ServerEnvelope {
        let data: Data
        switch message {
        case .data(let value): data = value
        case .string(let value): data = Data(value.utf8)
        @unknown default: throw AppServerClientError.invalidResponse
        }
        return try JSONDecoder().decode(ServerEnvelope.self, from: data)
    }

    private func jsonString(_ object: [String: Any]) throws -> String {
        let data = try JSONSerialization.data(withJSONObject: object)
        guard let value = String(data: data, encoding: .utf8) else {
            throw AppServerClientError.invalidResponse
        }
        return value
    }
}
