import Foundation
import BackgroundTasks
import UserNotifications

struct SavedAppConnection: Codable, Equatable, Sendable {
    let id: String
    let serverURL: String
}

struct AppConnection: Identifiable, Equatable, Sendable {
    let id: String
    let name: String
    let serverURL: String
    let status: String
    let isActive: Bool
}

@MainActor
final class AppModel: ObservableObject {
    enum ConnectionState: Equatable {
        case notConfigured
        case connecting
        case connected
        case failed(String)
    }

    @Published var serverURL: String
    @Published var operatorToken: String
    @Published private(set) var connectionState: ConnectionState
    @Published private(set) var health: AppServerHealth?
    @Published private(set) var providerCatalog: AppServerProviderCatalog?
    @Published private(set) var workspaces: [AppServerWorkspace] = []
    @Published private(set) var sessions: [AppServerSession] = []
    @Published private(set) var workspaceSessions: [String: [AppServerWorkspaceSession]] = [:]
    @Published private(set) var loadingWorkspaceSessions: Set<String> = []
    @Published private(set) var workspaceSessionErrors: [String: String] = [:]
    @Published private(set) var workspaceMemoryCatalogs: [String: AppServerWorkspaceMemoryCatalog] = [:]
    @Published private(set) var loadingWorkspaceMemory: Set<String> = []
    @Published private(set) var workspaceMemoryErrors: [String: String] = [:]
    @Published private(set) var sessionTranscripts: [String: [AppServerTranscriptMessage]] = [:]
    @Published private(set) var loadingSessionTranscripts: Set<String> = []
    @Published private(set) var sessionTranscriptErrors: [String: String] = [:]
    @Published private(set) var sessionCollaborations: [String: AppServerSessionCollaborationState] = [:]
    @Published private(set) var loadingSessionCollaborations: Set<String> = []
    @Published private(set) var sessionCollaborationErrors: [String: String] = [:]
    @Published private(set) var sendingSessionSteering: Set<String> = []
    @Published private(set) var sessionSteeringErrors: [String: String] = [:]
    @Published private(set) var stoppingSessions: Set<String> = []
    @Published private(set) var sessionStopErrors: [String: String] = [:]
    @Published private(set) var sessionPendingApprovals: [String: [AppServerPendingApproval]] = [:]
    @Published private(set) var sendingSessionApprovals: Set<String> = []
    @Published private(set) var sessionApprovalErrors: [String: String] = [:]
    @Published private(set) var memoryNotificationsEnabled: Bool
    @Published private(set) var memoryNotificationAuthorization: UNAuthorizationStatus = .notDetermined
    @Published private var savedConnections: [SavedAppConnection]

    private var sessionTranscriptCursors: [String: String] = [:]
    private var sessionControlChannels: [String: AppServerSessionControlChannel] = [:]
    private var resolvedSessionApprovalRequestIds: [String: Set<String>] = [:]
    private var refreshingMemoryNotificationScopes: Set<String> = []
    private var observedMemoryNotificationCheckpoints: [String: Set<String>]

    private let defaults: UserDefaults
    private let keychain: any OperatorTokenStore
    private let snapshotLoader: @Sendable (AppServerEndpoint, String) async throws -> AppServerSnapshot
    private var selectedConnectionID: String?
    private var pendingConnectionID: String?
    private let selectedConnectionIDKey = "BealeSelectedConnectionID"
    private let legacyServerURLKey = "BealeServerURL"
    private let memoryNotificationsEnabledKey = "BealeMemoryNotificationsEnabled"
    private let memoryNotificationStateKey = "BealeMemoryNotificationState"
    nonisolated static let memoryRefreshTaskIdentifier = "com.beale.ios.memory-refresh"

    init(
        defaults: UserDefaults = .standard,
        keychain: any OperatorTokenStore = KeychainStore(),
        snapshotLoader: @escaping @Sendable (AppServerEndpoint, String) async throws -> AppServerSnapshot = {
            endpoint,
            token in
            try await AppServerClient(endpoint: endpoint, operatorToken: token).fetchSnapshot()
        }
    ) {
        var storedConnections = Self.loadConnections(defaults: defaults)
        var storedSelectedConnectionID = defaults.string(forKey: "BealeSelectedConnectionID")

        if storedConnections.isEmpty,
           let legacyServerURL = defaults.string(forKey: "BealeServerURL"),
           let endpoint = try? AppServerEndpoint(legacyServerURL) {
            let migratedConnection = SavedAppConnection(
                id: UUID().uuidString,
                serverURL: endpoint.baseURL.absoluteString
            )
            let legacyToken = keychain.loadLegacyOperatorToken()
            do {
                try keychain.saveOperatorToken(legacyToken, connectionID: migratedConnection.id)
                keychain.deleteLegacyOperatorToken()
            } catch {
                // Keep the legacy credential available until a later successful migration.
            }
            storedConnections = [migratedConnection]
            storedSelectedConnectionID = migratedConnection.id
            Self.persistConnections(storedConnections, defaults: defaults)
            defaults.set(migratedConnection.id, forKey: "BealeSelectedConnectionID")
            defaults.removeObject(forKey: "BealeServerURL")
        }

        if storedSelectedConnectionID == nil
            || !storedConnections.contains(where: { $0.id == storedSelectedConnectionID }) {
            storedSelectedConnectionID = storedConnections.first?.id
        }
        let selectedConnection = storedConnections.first { $0.id == storedSelectedConnectionID }
        self.defaults = defaults
        self.keychain = keychain
        self.snapshotLoader = snapshotLoader
        savedConnections = storedConnections
        selectedConnectionID = storedSelectedConnectionID
        memoryNotificationsEnabled = defaults.bool(forKey: memoryNotificationsEnabledKey)
        observedMemoryNotificationCheckpoints = (defaults.dictionary(forKey: memoryNotificationStateKey) as? [String: [String]] ?? [:])
            .mapValues(Set.init)
        serverURL = selectedConnection?.serverURL ?? ""
        if let selectedConnection {
            let savedToken = keychain.loadOperatorToken(connectionID: selectedConnection.id)
            operatorToken = savedToken.isEmpty ? keychain.loadLegacyOperatorToken() : savedToken
        } else {
            operatorToken = ""
        }
        connectionState = .notConfigured
    }

    var isConnected: Bool {
        connectionState == .connected
    }

    var activeSessions: [AppServerSession] {
        sessions.filter(\.isActive)
    }

    var connections: [AppConnection] {
        savedConnections.map { connection in
            let isCurrent = connection.id == selectedConnectionID && connection.serverURL == serverURL
            let status: String
            if isCurrent {
                switch connectionState {
                case .notConfigured:
                    status = "Available"
                case .connecting:
                    status = "Connecting"
                case .connected:
                    status = "Connected"
                case .failed:
                    status = "Needs Attention"
                }
            } else {
                status = "Available"
            }
            return AppConnection(
                id: connection.id,
                name: URL(string: connection.serverURL)?.host ?? "Beale App Server",
                serverURL: connection.serverURL,
                status: status,
                isActive: isCurrent && isConnected
            )
        }
    }

    func operatorToken(for connection: AppConnection) -> String {
        guard savedConnections.contains(where: { $0.id == connection.id }) else { return "" }
        return keychain.loadOperatorToken(connectionID: connection.id)
    }

    func sessions(for workspace: AppServerWorkspace) -> [AppServerWorkspaceSession] {
        workspaceSessions[workspace.workspaceId] ?? []
    }

    func isLoadingSessions(for workspace: AppServerWorkspace) -> Bool {
        loadingWorkspaceSessions.contains(workspace.workspaceId)
    }

    func sessionError(for workspace: AppServerWorkspace) -> String? {
        workspaceSessionErrors[workspace.workspaceId]
    }

    func memoryCatalog(for workspace: AppServerWorkspace) -> AppServerWorkspaceMemoryCatalog? {
        workspaceMemoryCatalogs[workspace.workspaceId]
    }

    func isLoadingMemory(for workspace: AppServerWorkspace) -> Bool {
        loadingWorkspaceMemory.contains(workspace.workspaceId)
    }

    func memoryError(for workspace: AppServerWorkspace) -> String? {
        workspaceMemoryErrors[workspace.workspaceId]
    }

    func transcript(for session: AppServerWorkspaceSession) -> [AppServerTranscriptMessage] {
        AppServerTranscriptProjection.rootMessages(sessionTranscripts[session.id] ?? [])
    }

    func transcript(
        for session: AppServerWorkspaceSession,
        subagentPath: String
    ) -> [AppServerTranscriptMessage] {
        AppServerTranscriptProjection.subagentMessages(
            sessionTranscripts[session.id] ?? [],
            path: subagentPath
        )
    }

    func subagents(for session: AppServerWorkspaceSession) -> [AppServerSubagentSummary] {
        AppServerSubagentProjection.summaries(
            from: sessionCollaborations[session.id]?.subagents ?? [],
            sessionStatus: session.status
        )
    }

    func isLoadingCollaboration(for session: AppServerWorkspaceSession) -> Bool {
        loadingSessionCollaborations.contains(session.id)
    }

    func collaborationError(for session: AppServerWorkspaceSession) -> String? {
        sessionCollaborationErrors[session.id]
    }

    func isLoadingTranscript(for session: AppServerWorkspaceSession) -> Bool {
        loadingSessionTranscripts.contains(session.id)
    }

    func transcriptError(for session: AppServerWorkspaceSession) -> String? {
        sessionTranscriptErrors[session.id]
    }

    func isSendingSteering(for session: AppServerWorkspaceSession) -> Bool {
        sendingSessionSteering.contains(session.id)
    }

    func steeringError(for session: AppServerWorkspaceSession) -> String? {
        sessionSteeringErrors[session.id]
    }

    func isStopping(_ session: AppServerWorkspaceSession) -> Bool {
        stoppingSessions.contains(session.id)
    }

    func stopError(for session: AppServerWorkspaceSession) -> String? {
        sessionStopErrors[session.id]
    }

    func pendingApproval(for session: AppServerWorkspaceSession) -> AppServerPendingApproval? {
        sessionPendingApprovals[session.id]?.first
    }

    func isSendingApproval(for session: AppServerWorkspaceSession) -> Bool {
        sendingSessionApprovals.contains(session.id)
    }

    func approvalError(for session: AppServerWorkspaceSession) -> String? {
        sessionApprovalErrors[session.id]
    }

    func currentSession(_ session: AppServerWorkspaceSession, in workspace: AppServerWorkspace) -> AppServerWorkspaceSession {
        sessions(for: workspace).first { $0.id == session.id } ?? session
    }

    func connect() async {
        guard connectionState != .connecting else { return }
        connectionState = .connecting
        do {
            let endpoint = try AppServerEndpoint(serverURL)
            let token = operatorToken.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !token.isEmpty else {
                throw AppServerClientError.rejected(status: 401, message: "Enter the app-server operator token.")
            }
            let snapshot = try await snapshotLoader(endpoint, token)
            let connectionID = pendingConnectionID
                ?? savedConnections.first(where: { $0.serverURL == endpoint.baseURL.absoluteString })?.id
                ?? UUID().uuidString
            try keychain.saveOperatorToken(token, connectionID: connectionID)
            upsertConnection(id: connectionID, serverURL: endpoint.baseURL.absoluteString)
            selectedConnectionID = connectionID
            pendingConnectionID = nil
            defaults.set(connectionID, forKey: selectedConnectionIDKey)
            defaults.removeObject(forKey: legacyServerURLKey)
            keychain.deleteLegacyOperatorToken()
            serverURL = endpoint.baseURL.absoluteString
            operatorToken = token
            workspaceSessions = [:]
            loadingWorkspaceSessions = []
            workspaceSessionErrors = [:]
            clearMemoryState()
            clearTranscriptState()
            apply(snapshot)
            connectionState = .connected
            if memoryNotificationsEnabled {
                await refreshMemoryNotifications()
                scheduleBackgroundMemoryRefresh()
            }
        } catch {
            health = nil
            providerCatalog = nil
            workspaces = []
            sessions = []
            workspaceSessions = [:]
            loadingWorkspaceSessions = []
            workspaceSessionErrors = [:]
            clearMemoryState()
            clearTranscriptState()
            connectionState = .failed(connectionFailureDescription(error))
        }
    }

    func connect(to connection: AppConnection) async {
        guard connections.contains(where: { $0.id == connection.id }) else { return }
        selectedConnectionID = connection.id
        pendingConnectionID = connection.id
        serverURL = connection.serverURL
        operatorToken = keychain.loadOperatorToken(connectionID: connection.id)
        await connect()
    }

    func configureAndConnect(
        serverURL: String,
        operatorToken: String,
        replacing connectionID: String? = nil
    ) async {
        pendingConnectionID = connectionID
        self.serverURL = serverURL
        self.operatorToken = operatorToken
        await connect()
    }

    func connectOnLaunch() async {
        guard !serverURL.isEmpty, !operatorToken.isEmpty else {
            connectionState = .notConfigured
            return
        }
        connectionState = .notConfigured
        await connect()
    }

    func refresh() async {
        guard isConnected else {
            await connect()
            return
        }
        do {
            let endpoint = try AppServerEndpoint(serverURL)
            let snapshot = try await snapshotLoader(endpoint, operatorToken)
            apply(snapshot)
        } catch {
            connectionState = .failed(connectionFailureDescription(error))
        }
    }

    private func connectionFailureDescription(_ error: Error) -> String {
        let networkError = error as NSError
        guard networkError.domain == NSURLErrorDomain,
              networkError.code == URLError.cannotFindHost.rawValue
                || networkError.code == URLError.dnsLookupFailed.rawValue else {
            return error.localizedDescription
        }
        return "\(error.localizedDescription) Make sure Tailscale is connected on this iPhone and signed in to the same tailnet as the Mac, then try again."
    }

    func loadSessions(for workspace: AppServerWorkspace, force: Bool = false) async {
        let workspaceId = workspace.workspaceId
        guard isConnected,
              !loadingWorkspaceSessions.contains(workspaceId),
              force || workspaceSessions[workspaceId] == nil else {
            return
        }

        loadingWorkspaceSessions.insert(workspaceId)
        workspaceSessionErrors[workspaceId] = nil
        defer { loadingWorkspaceSessions.remove(workspaceId) }
        let requestedServerURL = serverURL
        let requestedToken = operatorToken

        do {
            let endpoint = try AppServerEndpoint(requestedServerURL)
            let client = AppServerClient(endpoint: endpoint, operatorToken: requestedToken)
            let fetchedSessions = try await client.fetchWorkspaceSessions(workspaceId: workspaceId)
            guard isConnected,
                  serverURL == endpoint.baseURL.absoluteString,
                  operatorToken == requestedToken else {
                return
            }
            workspaceSessions[workspaceId] = fetchedSessions
        } catch {
            guard isConnected,
                  serverURL == requestedServerURL,
                  operatorToken == requestedToken else {
                return
            }
            workspaceSessionErrors[workspaceId] = error.localizedDescription
        }
    }

    func researchSuggestions(
        in workspace: AppServerWorkspace,
        refresh: Bool = false
    ) async throws -> AppServerGeneratedResearchSuggestions {
        guard isConnected else {
            throw AppServerClientError.rejected(status: 503, message: "Connect to the app-server to load suggestions.")
        }
        let requestedServerURL = serverURL
        let requestedToken = operatorToken
        let client = try appServerClient(serverURL: requestedServerURL, token: requestedToken)
        let result = try await client.generateResearchSuggestions(
            workspaceId: workspace.workspaceId,
            refresh: refresh
        )
        guard connectionMatches(serverURL: requestedServerURL, token: requestedToken) else {
            throw CancellationError()
        }
        return result
    }

    func addContext(
        to rawPrompt: String,
        in workspace: AppServerWorkspace,
        phase: String?,
        configuration: AppServerSessionLaunchConfiguration
    ) async throws -> AppServerExpandedResearchPrompt {
        let prompt = rawPrompt.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !prompt.isEmpty else {
            throw AppServerClientError.rejected(status: 400, message: "Enter a research request before adding context.")
        }
        guard isConnected else {
            throw AppServerClientError.rejected(status: 503, message: "Connect to the app-server before adding context.")
        }
        let requestedServerURL = serverURL
        let requestedToken = operatorToken
        let client = try appServerClient(serverURL: requestedServerURL, token: requestedToken)
        let result = try await client.expandResearchPrompt(
            workspaceId: workspace.workspaceId,
            promptMarkdown: prompt,
            phase: phase,
            configuration: configuration
        )
        guard connectionMatches(serverURL: requestedServerURL, token: requestedToken) else {
            throw CancellationError()
        }
        return result
    }

    func startSession(
        in workspace: AppServerWorkspace,
        prompt rawPrompt: String,
        configuration: AppServerSessionLaunchConfiguration
    ) async throws -> AppServerWorkspaceSession {
        let prompt = rawPrompt.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !prompt.isEmpty else {
            throw AppServerClientError.rejected(status: 400, message: "Enter a research request.")
        }
        guard isConnected else {
            throw AppServerClientError.rejected(status: 503, message: "Connect to the app-server before starting research.")
        }

        let requestedServerURL = serverURL
        let requestedToken = operatorToken
        let client = try appServerClient(serverURL: requestedServerURL, token: requestedToken)
        let started = try await client.startSession(
            workspace: workspace,
            promptMarkdown: prompt,
            configuration: configuration
        )
        guard connectionMatches(serverURL: requestedServerURL, token: requestedToken) else {
            throw CancellationError()
        }

        sessions.removeAll { $0.id == started.session.id }
        sessions.insert(started.session, at: 0)

        let provisional = AppServerWorkspaceSession(
            id: started.session.sessionId,
            workspaceId: workspace.workspaceId,
            status: started.session.isActive ? "active" : started.session.state,
            title: "",
            prompt: prompt,
            startedAt: started.session.startedAt,
            updatedAt: started.session.endedAt ?? started.session.startedAt
        )
        workspaceSessions[workspace.workspaceId] = inserting(
            provisional,
            into: workspaceSessions[workspace.workspaceId] ?? []
        )

        do {
            let fetched = try await client.fetchWorkspaceSessions(workspaceId: workspace.workspaceId)
            guard connectionMatches(serverURL: requestedServerURL, token: requestedToken) else {
                throw CancellationError()
            }
            if let canonical = fetched.first(where: { $0.id == provisional.id }) {
                workspaceSessions[workspace.workspaceId] = fetched
                workspaceSessionErrors[workspace.workspaceId] = nil
                return canonical
            }
            workspaceSessions[workspace.workspaceId] = inserting(provisional, into: fetched)
        } catch is CancellationError {
            throw CancellationError()
        } catch {
            // The session is already running. Keep its provisional row until the next canonical refresh.
        }
        return provisional
    }

    func refreshWorkspaces() async {
        await refresh()
        guard isConnected else { return }
        for workspace in workspaces {
            await loadSessions(for: workspace, force: true)
            await loadMemory(for: workspace, force: true)
        }
    }

    func loadMemory(for workspace: AppServerWorkspace, force: Bool = false) async {
        let workspaceId = workspace.workspaceId
        guard isConnected,
              !loadingWorkspaceMemory.contains(workspaceId),
              force || workspaceMemoryCatalogs[workspaceId] == nil else {
            return
        }

        loadingWorkspaceMemory.insert(workspaceId)
        workspaceMemoryErrors[workspaceId] = nil
        defer { loadingWorkspaceMemory.remove(workspaceId) }
        let requestedServerURL = serverURL
        let requestedToken = operatorToken
        do {
            let client = try appServerClient(serverURL: requestedServerURL, token: requestedToken)
            let catalog = try await client.fetchWorkspaceMemory(workspaceId: workspaceId)
            guard connectionMatches(serverURL: requestedServerURL, token: requestedToken) else { return }
            workspaceMemoryCatalogs[workspaceId] = catalog
        } catch {
            guard connectionMatches(serverURL: requestedServerURL, token: requestedToken) else { return }
            workspaceMemoryErrors[workspaceId] = error.localizedDescription
        }
    }

    func markHistoryDuplicate(
        in workspace: AppServerWorkspace,
        type: String,
        id: String,
        parentId: String,
        expectedRevision: Int
    ) async throws {
        guard isConnected else {
            throw AppServerClientError.rejected(status: 503, message: "Connect to the app-server before changing workspace history.")
        }
        let requestedServerURL = serverURL
        let requestedToken = operatorToken
        let client = try appServerClient(serverURL: requestedServerURL, token: requestedToken)
        try await client.markHistoryDuplicate(
            workspaceId: workspace.workspaceId,
            type: type,
            id: id,
            parentId: parentId,
            expectedRevision: expectedRevision
        )
        guard connectionMatches(serverURL: requestedServerURL, token: requestedToken) else {
            throw CancellationError()
        }
        await loadMemory(for: workspace, force: true)
    }

    func undoHistoryDuplicate(
        in workspace: AppServerWorkspace,
        type: String,
        id: String,
        expectedRevision: Int
    ) async throws {
        guard isConnected else {
            throw AppServerClientError.rejected(status: 503, message: "Connect to the app-server before changing workspace history.")
        }
        let requestedServerURL = serverURL
        let requestedToken = operatorToken
        let client = try appServerClient(serverURL: requestedServerURL, token: requestedToken)
        try await client.undoHistoryDuplicate(
            workspaceId: workspace.workspaceId,
            type: type,
            id: id,
            expectedRevision: expectedRevision
        )
        guard connectionMatches(serverURL: requestedServerURL, token: requestedToken) else {
            throw CancellationError()
        }
        await loadMemory(for: workspace, force: true)
    }

    func loadTranscript(
        for session: AppServerWorkspaceSession,
        in workspace: AppServerWorkspace,
        force: Bool = false
    ) async {
        let sessionId = session.id
        guard isConnected,
              !loadingSessionTranscripts.contains(sessionId),
              force || sessionTranscripts[sessionId] == nil else {
            return
        }

        loadingSessionTranscripts.insert(sessionId)
        sessionTranscriptErrors[sessionId] = nil
        defer { loadingSessionTranscripts.remove(sessionId) }
        let requestedServerURL = serverURL
        let requestedToken = operatorToken

        do {
            let client = try appServerClient(serverURL: requestedServerURL, token: requestedToken)
            let page = try await client.fetchSessionTranscript(
                workspaceId: workspace.workspaceId,
                sessionId: sessionId,
                tail: true
            )
            guard connectionMatches(serverURL: requestedServerURL, token: requestedToken) else { return }
            applyTranscriptPage(page, sessionId: sessionId, replacing: true)
        } catch {
            guard connectionMatches(serverURL: requestedServerURL, token: requestedToken) else { return }
            sessionTranscriptErrors[sessionId] = error.localizedDescription
        }
    }

    func followSession(_ session: AppServerWorkspaceSession, in workspace: AppServerWorkspace) async {
        await loadTranscript(for: session, in: workspace)
        await loadCollaboration(for: session, in: workspace)
        await loadApprovals(for: session, in: workspace)
        await refreshMemoryNotifications(for: workspace)
        guard currentSession(session, in: workspace).status == "active" else { return }

        var pollCount = 0
        while !Task.isCancelled {
            do {
                try await Task.sleep(for: .seconds(2))
            } catch {
                return
            }
            guard !Task.isCancelled else { return }
            await pollTranscript(for: session, in: workspace)
            await loadCollaboration(for: session, in: workspace, force: true)
            await loadApprovals(for: session, in: workspace)
            pollCount += 1
            if pollCount.isMultiple(of: 3) {
                await loadSessions(for: workspace, force: true)
                await loadMemory(for: workspace, force: true)
                await refreshMemoryNotifications(for: workspace)
                if currentSession(session, in: workspace).status != "active" {
                    sessionPendingApprovals[session.id] = []
                    await pollTranscript(for: session, in: workspace)
                    return
                }
            }
        }
    }

    func refreshSession(_ session: AppServerWorkspaceSession, in workspace: AppServerWorkspace) async {
        await loadTranscript(for: session, in: workspace, force: true)
        await loadCollaboration(for: session, in: workspace, force: true)
        await loadApprovals(for: session, in: workspace)
        await loadSessions(for: workspace, force: true)
        await loadMemory(for: workspace, force: true)
        await refreshMemoryNotifications(for: workspace)
    }

    func loadCollaboration(
        for session: AppServerWorkspaceSession,
        in workspace: AppServerWorkspace,
        force: Bool = false
    ) async {
        let sessionId = session.id
        guard isConnected,
              !loadingSessionCollaborations.contains(sessionId),
              force || sessionCollaborations[sessionId] == nil else {
            return
        }
        loadingSessionCollaborations.insert(sessionId)
        sessionCollaborationErrors[sessionId] = nil
        defer { loadingSessionCollaborations.remove(sessionId) }
        let requestedServerURL = serverURL
        let requestedToken = operatorToken
        do {
            let client = try appServerClient(serverURL: requestedServerURL, token: requestedToken)
            let collaboration = try await client.fetchSessionCollaboration(
                workspaceId: workspace.workspaceId,
                sessionId: sessionId
            )
            guard connectionMatches(serverURL: requestedServerURL, token: requestedToken) else { return }
            sessionCollaborations[sessionId] = collaboration
        } catch {
            guard connectionMatches(serverURL: requestedServerURL, token: requestedToken) else { return }
            sessionCollaborationErrors[sessionId] = error.localizedDescription
        }
    }

    func loadApprovals(for session: AppServerWorkspaceSession, in workspace: AppServerWorkspace) async {
        let sessionId = session.id
        guard isConnected, currentSession(session, in: workspace).status == "active" else {
            sessionPendingApprovals[sessionId] = []
            resolvedSessionApprovalRequestIds[sessionId] = nil
            return
        }
        let requestedServerURL = serverURL
        let requestedToken = operatorToken
        do {
            let client = try appServerClient(serverURL: requestedServerURL, token: requestedToken)
            let page = try await client.fetchSessionApprovalEvents(
                workspaceId: workspace.workspaceId,
                sessionId: sessionId
            )
            guard connectionMatches(serverURL: requestedServerURL, token: requestedToken) else { return }
            let projected = AppServerApprovalProjection.pendingApprovals(page.events)
            let projectedIds = Set(projected.map(\.id))
            let suppressedIds = (resolvedSessionApprovalRequestIds[sessionId] ?? []).intersection(projectedIds)
            resolvedSessionApprovalRequestIds[sessionId] = suppressedIds.isEmpty ? nil : suppressedIds
            sessionPendingApprovals[sessionId] = projected.filter { !suppressedIds.contains($0.id) }
            sessionApprovalErrors[sessionId] = nil
        } catch {
            guard connectionMatches(serverURL: requestedServerURL, token: requestedToken) else { return }
            sessionApprovalErrors[sessionId] = error.localizedDescription
        }
    }

    func sendSteering(
        _ rawInstruction: String,
        to session: AppServerWorkspaceSession,
        in workspace: AppServerWorkspace
    ) async -> Bool {
        let instruction = rawInstruction.trimmingCharacters(in: .whitespacesAndNewlines)
        let sessionId = session.id
        guard !instruction.isEmpty,
              currentSession(session, in: workspace).status == "active",
              !sendingSessionSteering.contains(sessionId) else {
            return false
        }

        sendingSessionSteering.insert(sessionId)
        sessionSteeringErrors[sessionId] = nil
        defer { sendingSessionSteering.remove(sessionId) }
        do {
            let requestId = try await sendSteeringOverChannel(instruction, sessionId: sessionId)
            let optimistic = AppServerTranscriptMessage(
                id: "transcript_steering_\(requestId)",
                runId: sessionId,
                attemptId: nil,
                traceEventId: nil,
                role: "user",
                phase: nil,
                contentMarkdown: instruction,
                source: "user_steering",
                metadata: nil,
                createdAt: ISO8601DateFormatter().string(from: Date())
            )
            var messages = sessionTranscripts[sessionId] ?? []
            messages.removeAll { $0.id == optimistic.id }
            messages.append(optimistic)
            sessionTranscripts[sessionId] = messages.sorted {
                $0.createdAt == $1.createdAt ? $0.id < $1.id : $0.createdAt < $1.createdAt
            }
            return true
        } catch {
            sessionControlChannels.removeValue(forKey: sessionId)?.close()
            sessionSteeringErrors[sessionId] = error.localizedDescription
            return false
        }
    }

    func stopSession(
        _ session: AppServerWorkspaceSession,
        in workspace: AppServerWorkspace
    ) async -> Bool {
        let sessionId = session.id
        guard currentSession(session, in: workspace).status == "active",
              !stoppingSessions.contains(sessionId) else {
            return false
        }

        stoppingSessions.insert(sessionId)
        sessionStopErrors[sessionId] = nil
        defer { stoppingSessions.remove(sessionId) }
        let requestedServerURL = serverURL
        let requestedToken = operatorToken

        do {
            let client = try appServerClient(serverURL: requestedServerURL, token: requestedToken)
            _ = try await client.stopSession(sessionId: sessionId)
            guard connectionMatches(serverURL: requestedServerURL, token: requestedToken) else {
                throw CancellationError()
            }

            sessionControlChannels.removeValue(forKey: sessionId)?.close()
            sendingSessionSteering.remove(sessionId)
            sessionSteeringErrors[sessionId] = nil
            sessionPendingApprovals[sessionId] = []
            sendingSessionApprovals.remove(sessionId)
            sessionApprovalErrors[sessionId] = nil
            resolvedSessionApprovalRequestIds[sessionId] = nil
            sessions.removeAll { $0.sessionId == sessionId }

            let current = currentSession(session, in: workspace)
            let stopped = AppServerWorkspaceSession(
                id: current.id,
                workspaceId: current.workspaceId,
                status: "stopped",
                title: current.title,
                prompt: current.prompt,
                startedAt: current.startedAt,
                updatedAt: ISO8601DateFormatter().string(from: Date())
            )
            workspaceSessions[workspace.workspaceId] = inserting(
                stopped,
                into: workspaceSessions[workspace.workspaceId] ?? []
            )
            return true
        } catch is CancellationError {
            return false
        } catch {
            guard connectionMatches(serverURL: requestedServerURL, token: requestedToken) else {
                return false
            }
            sessionStopErrors[sessionId] = error.localizedDescription
            return false
        }
    }

    func resolveApproval(
        _ approval: AppServerPendingApproval,
        decision: String,
        for session: AppServerWorkspaceSession,
        in workspace: AppServerWorkspace
    ) async {
        let sessionId = session.id
        guard currentSession(session, in: workspace).status == "active",
              !sendingSessionApprovals.contains(sessionId) else {
            return
        }
        sendingSessionApprovals.insert(sessionId)
        sessionApprovalErrors[sessionId] = nil
        defer { sendingSessionApprovals.remove(sessionId) }
        do {
            let channel = try await sessionControlChannel(sessionId: sessionId)
            _ = try await channel.sendApproval(
                approvalRequestId: approval.id,
                requestKind: approval.requestKind,
                decision: decision
            )
            resolvedSessionApprovalRequestIds[sessionId, default: []].insert(approval.id)
            sessionPendingApprovals[sessionId]?.removeAll { $0.id == approval.id }
        } catch {
            sessionControlChannels.removeValue(forKey: sessionId)?.close()
            sessionApprovalErrors[sessionId] = error.localizedDescription
        }
    }

    func refreshMemoryNotificationAuthorization() async {
        memoryNotificationAuthorization = await UNUserNotificationCenter.current().notificationSettings().authorizationStatus
        if memoryNotificationAuthorization == .denied, memoryNotificationsEnabled {
            memoryNotificationsEnabled = false
            defaults.set(false, forKey: memoryNotificationsEnabledKey)
            BGTaskScheduler.shared.cancel(taskRequestWithIdentifier: Self.memoryRefreshTaskIdentifier)
        }
    }

    func setMemoryNotificationsEnabled(_ enabled: Bool) async {
        if !enabled {
            memoryNotificationsEnabled = false
            defaults.set(false, forKey: memoryNotificationsEnabledKey)
            BGTaskScheduler.shared.cancel(taskRequestWithIdentifier: Self.memoryRefreshTaskIdentifier)
            await refreshMemoryNotificationAuthorization()
            return
        }

        do {
            _ = try await UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound])
        } catch {
            memoryNotificationsEnabled = false
            defaults.set(false, forKey: memoryNotificationsEnabledKey)
            await refreshMemoryNotificationAuthorization()
            return
        }
        await refreshMemoryNotificationAuthorization()
        guard memoryNotificationAuthorization == .authorized
                || memoryNotificationAuthorization == .provisional
                || memoryNotificationAuthorization == .ephemeral else {
            memoryNotificationsEnabled = false
            defaults.set(false, forKey: memoryNotificationsEnabledKey)
            return
        }
        memoryNotificationsEnabled = true
        defaults.set(true, forKey: memoryNotificationsEnabledKey)
        await refreshMemoryNotifications()
        scheduleBackgroundMemoryRefresh()
    }

    func performBackgroundMemoryRefresh() async {
        guard memoryNotificationsEnabled else { return }
        if !isConnected {
            await connectOnLaunch()
        }
        if isConnected {
            await refreshMemoryNotifications()
        }
        scheduleBackgroundMemoryRefresh()
    }

    func followMemoryNotifications() async {
        guard memoryNotificationsEnabled, isConnected else { return }
        await refreshMemoryNotifications()
        while !Task.isCancelled, memoryNotificationsEnabled, isConnected {
            do {
                try await Task.sleep(for: .seconds(15))
            } catch {
                return
            }
            await refreshMemoryNotifications()
        }
    }

    func scheduleBackgroundMemoryRefresh() {
        guard memoryNotificationsEnabled else { return }
        let request = BGAppRefreshTaskRequest(identifier: Self.memoryRefreshTaskIdentifier)
        request.earliestBeginDate = Date(timeIntervalSinceNow: 15 * 60)
        BGTaskScheduler.shared.cancel(taskRequestWithIdentifier: Self.memoryRefreshTaskIdentifier)
        try? BGTaskScheduler.shared.submit(request)
    }

    func forgetConnection(_ connection: AppConnection) {
        guard let removedIndex = savedConnections.firstIndex(where: { $0.id == connection.id }) else { return }
        let removedCurrentConnection = connection.id == selectedConnectionID
        savedConnections.remove(at: removedIndex)
        persistConnections()
        keychain.deleteOperatorToken(connectionID: connection.id)

        guard removedCurrentConnection else { return }
        closeSessionControlChannels()
        selectedConnectionID = nil
        pendingConnectionID = nil
        defaults.removeObject(forKey: selectedConnectionIDKey)
        serverURL = ""
        operatorToken = ""
        health = nil
        providerCatalog = nil
        workspaces = []
        sessions = []
        workspaceSessions = [:]
        loadingWorkspaceSessions = []
        workspaceSessionErrors = [:]
        clearMemoryState()
        clearTranscriptState()
        connectionState = .notConfigured
    }

    private func upsertConnection(id: String, serverURL: String) {
        let savedConnection = SavedAppConnection(id: id, serverURL: serverURL)
        let supersededIDs = savedConnections
            .filter { $0.serverURL == serverURL && $0.id != id }
            .map(\.id)
        for supersededID in supersededIDs {
            keychain.deleteOperatorToken(connectionID: supersededID)
        }
        savedConnections.removeAll { $0.id == id || ($0.serverURL == serverURL && $0.id != id) }
        savedConnections.append(savedConnection)
        persistConnections()
    }

    private func persistConnections() {
        Self.persistConnections(savedConnections, defaults: defaults)
    }

    private static func loadConnections(defaults: UserDefaults) -> [SavedAppConnection] {
        guard let data = defaults.data(forKey: "BealeConnections"),
              let connections = try? JSONDecoder().decode([SavedAppConnection].self, from: data) else {
            return []
        }
        var seenIDs: Set<String> = []
        var seenURLs: Set<String> = []
        return connections.filter { connection in
            guard !connection.id.isEmpty,
                  (try? AppServerEndpoint(connection.serverURL)) != nil,
                  seenIDs.insert(connection.id).inserted,
                  seenURLs.insert(connection.serverURL).inserted else {
                return false
            }
            return true
        }
    }

    private static func persistConnections(_ connections: [SavedAppConnection], defaults: UserDefaults) {
        guard let data = try? JSONEncoder().encode(connections) else { return }
        defaults.set(data, forKey: "BealeConnections")
    }

    private func apply(_ snapshot: AppServerSnapshot) {
        health = snapshot.health
        providerCatalog = snapshot.providerCatalog
        workspaces = snapshot.workspaces.sorted {
            $0.name.localizedCaseInsensitiveCompare($1.name) == .orderedAscending
        }
        sessions = snapshot.sessions.sorted { $0.startedAt > $1.startedAt }
        let knownWorkspaceIds = Set(workspaces.map(\.workspaceId))
        workspaceSessions = workspaceSessions.filter { knownWorkspaceIds.contains($0.key) }
        workspaceSessionErrors = workspaceSessionErrors.filter { knownWorkspaceIds.contains($0.key) }
        workspaceMemoryCatalogs = workspaceMemoryCatalogs.filter { knownWorkspaceIds.contains($0.key) }
        workspaceMemoryErrors = workspaceMemoryErrors.filter { knownWorkspaceIds.contains($0.key) }
    }

    private func refreshMemoryNotifications(for workspace: AppServerWorkspace? = nil) async {
        guard memoryNotificationsEnabled, isConnected else { return }
        let requestedServerURL = serverURL
        let requestedToken = operatorToken
        let targets = workspace.map { [$0] } ?? workspaces

        for target in targets {
            let scope = memoryNotificationScope(serverURL: requestedServerURL, workspaceId: target.workspaceId)
            guard !refreshingMemoryNotificationScopes.contains(scope) else { continue }
            refreshingMemoryNotificationScopes.insert(scope)
            defer { refreshingMemoryNotificationScopes.remove(scope) }

            do {
                let client = try appServerClient(serverURL: requestedServerURL, token: requestedToken)
                let feed = try await client.fetchMemoryNotifications(workspaceId: target.workspaceId)
                guard connectionMatches(serverURL: requestedServerURL, token: requestedToken) else { return }
                await applyMemoryNotificationFeed(feed, workspace: target, scope: scope)
            } catch {
                continue
            }
        }
    }

    private func applyMemoryNotificationFeed(
        _ feed: AppServerMemoryNotificationFeed,
        workspace: AppServerWorkspace,
        scope: String
    ) async {
        let currentCheckpoints = Set(feed.nodes.map(\.notificationCheckpoint))
        guard let observedCheckpoints = observedMemoryNotificationCheckpoints[scope] else {
            observedMemoryNotificationCheckpoints[scope] = currentCheckpoints
            persistMemoryNotificationState()
            return
        }

        let newNodes = feed.nodes
            .filter {
                $0.isEligibleForIOSNotification
                    && !observedCheckpoints.contains($0.notificationCheckpoint)
                    && !observedCheckpoints.contains($0.id) // v1 persisted checkpoint
            }
            .sorted { $0.createdAt == $1.createdAt ? $0.id < $1.id : $0.createdAt < $1.createdAt }
        observedMemoryNotificationCheckpoints[scope] = currentCheckpoints
        persistMemoryNotificationState()

        for node in newNodes {
            await BealeNotificationPresenter.shared.present(node: node, workspace: workspace)
        }
    }

    private func memoryNotificationScope(serverURL: String, workspaceId: String) -> String {
        "\(serverURL)|\(workspaceId)"
    }

    private func persistMemoryNotificationState() {
        defaults.set(observedMemoryNotificationCheckpoints.mapValues { Array($0).sorted() }, forKey: memoryNotificationStateKey)
    }

    private func pollTranscript(for session: AppServerWorkspaceSession, in workspace: AppServerWorkspace) async {
        let sessionId = session.id
        guard isConnected, !loadingSessionTranscripts.contains(sessionId) else { return }
        let requestedServerURL = serverURL
        let requestedToken = operatorToken
        var cursor = sessionTranscriptCursors[sessionId]
        var pageCount = 0

        do {
            let client = try appServerClient(serverURL: requestedServerURL, token: requestedToken)
            repeat {
                let page = try await client.fetchSessionTranscript(
                    workspaceId: workspace.workspaceId,
                    sessionId: sessionId,
                    afterEventId: cursor
                )
                guard connectionMatches(serverURL: requestedServerURL, token: requestedToken) else { return }
                applyTranscriptPage(page, sessionId: sessionId, replacing: false)
                cursor = page.nextAfterEventId
                pageCount += 1
                if !page.hasMore || pageCount >= 5 { break }
            } while !Task.isCancelled
            sessionTranscriptErrors[sessionId] = nil
        } catch {
            guard connectionMatches(serverURL: requestedServerURL, token: requestedToken) else { return }
            sessionTranscriptErrors[sessionId] = error.localizedDescription
        }
    }

    private func applyTranscriptPage(
        _ page: AppServerSessionEventPage,
        sessionId: String,
        replacing: Bool
    ) {
        let incoming = page.events
            .filter { $0.kind == "beale.transcript" || $0.kind == "beale.tool_summary" }
            .compactMap(\.payload.record)
        var messagesById = Dictionary(
            uniqueKeysWithValues: (replacing ? [] : sessionTranscripts[sessionId] ?? []).map { ($0.id, $0) }
        )
        for message in incoming {
            messagesById[message.id] = message
        }
        let sortedMessages = messagesById.values.sorted {
            $0.createdAt == $1.createdAt ? $0.id < $1.id : $0.createdAt < $1.createdAt
        }
        sessionTranscripts[sessionId] = sortedMessages
        if let cursor = page.nextAfterEventId {
            sessionTranscriptCursors[sessionId] = cursor
        } else if replacing {
            sessionTranscriptCursors[sessionId] = nil
        }
    }

    private func appServerClient(serverURL: String, token: String) throws -> AppServerClient {
        AppServerClient(endpoint: try AppServerEndpoint(serverURL), operatorToken: token)
    }

    private func sendSteeringOverChannel(_ instruction: String, sessionId: String) async throws -> String {
        try await sessionControlChannel(sessionId: sessionId).sendSteering(instruction)
    }

    private func sessionControlChannel(sessionId: String) async throws -> AppServerSessionControlChannel {
        if let channel = sessionControlChannels[sessionId], channel.isConnected {
            return channel
        }
        sessionControlChannels.removeValue(forKey: sessionId)?.close()
        let endpoint = try AppServerEndpoint(serverURL)
        let client = AppServerClient(endpoint: endpoint, operatorToken: operatorToken)
        let attachment = try await client.createSessionAttachment(sessionId: sessionId)
        let channel = AppServerSessionControlChannel(
            endpoint: endpoint,
            sessionId: sessionId,
            transport: attachment.transport
        )
        try await channel.connect()
        sessionControlChannels[sessionId] = channel
        return channel
    }

    private func connectionMatches(serverURL requestedServerURL: String, token: String) -> Bool {
        isConnected && serverURL == requestedServerURL && operatorToken == token
    }

    private func inserting(
        _ session: AppServerWorkspaceSession,
        into sessions: [AppServerWorkspaceSession]
    ) -> [AppServerWorkspaceSession] {
        ([session] + sessions.filter { $0.id != session.id }).sorted {
            $0.updatedAt == $1.updatedAt ? $0.id < $1.id : $0.updatedAt > $1.updatedAt
        }
    }

    private func clearTranscriptState() {
        closeSessionControlChannels()
        sessionTranscripts = [:]
        loadingSessionTranscripts = []
        sessionTranscriptErrors = [:]
        sessionTranscriptCursors = [:]
        sessionCollaborations = [:]
        loadingSessionCollaborations = []
        sessionCollaborationErrors = [:]
        sendingSessionSteering = []
        sessionSteeringErrors = [:]
        stoppingSessions = []
        sessionStopErrors = [:]
        sessionPendingApprovals = [:]
        sendingSessionApprovals = []
        sessionApprovalErrors = [:]
        resolvedSessionApprovalRequestIds = [:]
    }

    private func clearMemoryState() {
        workspaceMemoryCatalogs = [:]
        loadingWorkspaceMemory = []
        workspaceMemoryErrors = [:]
    }

    private func closeSessionControlChannels() {
        for channel in sessionControlChannels.values {
            channel.close()
        }
        sessionControlChannels = [:]
    }
}

final class BealeNotificationPresenter: NSObject, UNUserNotificationCenterDelegate {
    static let shared = BealeNotificationPresenter()

    private override init() {
        super.init()
    }

    func present(node: AppServerMemoryNotificationNode, workspace: AppServerWorkspace) async {
        let content = UNMutableNotificationContent()
        content.title = "\((node.rating ?? node.heat).capitalized) \(node.typeName)"
        content.subtitle = workspace.name
        content.body = node.summary.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            ? node.title
            : node.summary
        content.sound = .default
        content.threadIdentifier = workspace.workspaceId
        content.userInfo = [
            "workspaceId": workspace.workspaceId,
            "memoryId": node.id,
            "sessionId": node.sessionIds.first ?? "",
        ]
        let request = UNNotificationRequest(
            identifier: "beale-attention-\(workspace.workspaceId)-\(node.id)-\(node.revision)",
            content: content,
            trigger: nil
        )
        try? await UNUserNotificationCenter.current().add(request)
    }

    func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        willPresent notification: UNNotification
    ) async -> UNNotificationPresentationOptions {
        [.banner, .sound]
    }
}
