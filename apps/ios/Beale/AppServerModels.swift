import Foundation

enum AppServerPairingError: LocalizedError, Equatable {
    case invalidCode
    case unsupportedVersion
    case invalidToken

    var errorDescription: String? {
        switch self {
        case .invalidCode:
            return "This is not a Beale app-server QR code."
        case .unsupportedVersion:
            return "This Beale app-server QR code uses an unsupported version."
        case .invalidToken:
            return "The QR code does not contain a valid operator token."
        }
    }
}

struct AppServerPairingPayload: Equatable, Sendable {
    let serverURL: String
    let operatorToken: String

    init(scannedValue: String) throws {
        guard let components = URLComponents(string: scannedValue),
              components.scheme?.lowercased() == "beale",
              components.host?.lowercased() == "connect",
              components.path.isEmpty,
              components.fragment == nil else {
            throw AppServerPairingError.invalidCode
        }
        let values = Dictionary(grouping: components.queryItems ?? [], by: \.name)
        guard values["v"]?.count == 1,
              values["url"]?.count == 1,
              values["token"]?.count == 1,
              values.count == 3 else {
            throw AppServerPairingError.invalidCode
        }
        guard values["v"]?.first?.value == "1" else {
            throw AppServerPairingError.unsupportedVersion
        }
        guard let rawURL = values["url"]?.first?.value else {
            throw AppServerPairingError.invalidCode
        }
        let endpoint = try AppServerEndpoint(rawURL)
        guard let token = values["token"]?.first?.value?.trimmingCharacters(in: .whitespacesAndNewlines),
              !token.isEmpty,
              token.count <= 512,
              token.unicodeScalars.allSatisfy({ !CharacterSet.controlCharacters.contains($0) }) else {
            throw AppServerPairingError.invalidToken
        }
        serverURL = endpoint.baseURL.absoluteString
        operatorToken = token
    }
}

enum BealeAppServerContract {
    static let controlVersion = 1
    static let sessionLaunchVersion = 2
    static let appServerProtocolVersion = 1
    static let memoryNotificationSchemaVersion = 3
    static let workspaceMemorySchemaVersion = 4

    static let requiredCapabilities: Set<String> = [
        "session.typed-launch.v2",
        "session.exit-diagnostics",
        "session.transport-path.v1",
        "session.reconnect.v1",
        "session.multi-client.v1",
        "host.control.v1",
        "host.descriptor.v1",
        "host.provider-catalog.v1",
        "canonical.reads.v1",
        "session.commentary.v1",
        "memory.notifications.v3",
        "knowledge.claims.v2",
        "knowledge.claim-deduplication.v1",
        "knowledge.history-deduplication.v1",
        "workspace.goal-suggestions.v1",
        "workspace.prompt-expansion.v1"
    ]
}

struct AppServerHealth: Decodable, Sendable {
    let ok: Bool
    let controlVersion: Int
    let contractTimestamp: String
    let capabilities: [String]

    func validateCompatibility() throws {
        guard ok else {
            throw AppServerClientError.incompatible("The server did not report a healthy state.")
        }
        guard controlVersion == BealeAppServerContract.controlVersion else {
            throw AppServerClientError.incompatible(
                "Control version \(controlVersion) is not supported by this app."
            )
        }
        let missing = BealeAppServerContract.requiredCapabilities.subtracting(capabilities)
        guard missing.isEmpty else {
            throw AppServerClientError.incompatible(
                "The app-server is missing required capabilities: \(missing.sorted().joined(separator: ", "))."
            )
        }
    }
}

struct AppServerDescriptor: Decodable, Sendable {
    let sessionLaunchVersion: Int
    let appServerProtocolVersion: Int

    func validateCompatibility() throws {
        guard sessionLaunchVersion == BealeAppServerContract.sessionLaunchVersion else {
            throw AppServerClientError.incompatible(
                "Session launch version \(sessionLaunchVersion) is not supported by this app."
            )
        }
        guard appServerProtocolVersion == BealeAppServerContract.appServerProtocolVersion else {
            throw AppServerClientError.incompatible(
                "app-server protocol version \(appServerProtocolVersion) is not supported by this app."
            )
        }
    }
}

struct AppServerProviderModel: Decodable, Identifiable, Sendable {
    let id: String
    let name: String
    let reasoning: Bool
    let effortLevels: [String]
}

struct AppServerProviderCatalogEntry: Decodable, Identifiable, Sendable {
    let providerId: String
    let providerName: String
    let defaultLeadModel: String?
    let defaultSubagentModel: String?
    let defaultReasoningEffort: String?
    let models: [AppServerProviderModel]

    var id: String { providerId }
}

struct AppServerProviderCatalog: Decodable, Sendable {
    let controlVersion: Int
    let defaultProviderId: String?
    let providers: [AppServerProviderCatalogEntry]

    func validateCompatibility() throws {
        guard controlVersion == BealeAppServerContract.controlVersion,
              providers.allSatisfy({ provider in
                  !provider.providerId.isEmpty
                      && !provider.providerName.isEmpty
                      && !provider.models.isEmpty
                      && provider.models.allSatisfy { !$0.id.isEmpty && !$0.name.isEmpty }
              }) else {
            throw AppServerClientError.incompatible("The app-server returned an invalid provider model catalog.")
        }
    }
}

struct AppServerWorkspaceList: Decodable, Sendable {
    let controlVersion: Int
    let workspaces: [AppServerWorkspace]
}

struct AppServerWorkspace: Decodable, Identifiable, Sendable {
    let id: String
    let workspaceId: String
    let name: String
    let researchProfileId: String
    let researchKitId: String
    let runCount: Int
    let lastRunAt: String?
    let updatedAt: String
}

struct AppServerCanonicalWorkspaceSessions: Decodable, Sendable {
    let controlVersion: Int
    let workspace: AppServerWorkspace
    let result: [AppServerWorkspaceSession]
}

struct AppServerCanonicalMemoryNotifications: Decodable, Sendable {
    let controlVersion: Int
    let workspace: AppServerWorkspace
    let result: AppServerMemoryNotificationFeed

    func validatedResult(workspaceId: String) throws -> AppServerMemoryNotificationFeed {
        guard controlVersion == BealeAppServerContract.controlVersion,
              workspace.workspaceId == workspaceId,
              result.schemaVersion == BealeAppServerContract.memoryNotificationSchemaVersion,
              result.workspaceId == workspaceId else {
            throw AppServerClientError.incompatible("The memory notification response did not match this request.")
        }
        return result
    }
}

struct AppServerCanonicalWorkspaceMemory: Decodable, Sendable {
    let controlVersion: Int
    let workspace: AppServerWorkspace
    let result: AppServerWorkspaceMemoryCatalog

    func validatedResult(workspaceId: String) throws -> AppServerWorkspaceMemoryCatalog {
        guard controlVersion == BealeAppServerContract.controlVersion,
              workspace.workspaceId == workspaceId,
              result.schemaVersion == BealeAppServerContract.workspaceMemorySchemaVersion,
              result.workspaceId == workspaceId else {
            throw AppServerClientError.incompatible("The workspace memory response did not match this request.")
        }
        return result
    }
}

struct AppServerWorkspaceMemoryCatalog: Decodable, Sendable {
    let schemaVersion: Int
    let workspaceId: String
    let status: String
    let nodeCount: Int
    let nodeTypeCounts: [String: Int]
    let nodes: [AppServerWorkspaceMemoryNode]
    let leads: [AppServerWorkspaceResearchClaim]
    let findings: [AppServerWorkspaceResearchClaim]
}

struct AppServerWorkspaceResearchClaim: Decodable, Identifiable, Sendable {
    let id: String
    let sessionIds: [String]
    let projection: String
    let maturity: String
    let freshness: String
    let workflow: String
    let rating: String
    let classification: String
    let componentClaimIds: [String]
    let duplicateClaims: [AppServerWorkspaceResearchClaimDuplicate]
    let title: String
    let summary: String
    let impact: String
    let confidence: Double
    let evidenceCount: Int
    let createdAt: String
    let updatedAt: String
    let revision: Int
}

struct AppServerWorkspaceResearchClaimDuplicate: Decodable, Identifiable, Sendable {
    let id: String
    let projection: String
    let maturity: String
    let rating: String
    let classification: String
    let title: String
    let status: String
    let revision: Int
    let markedAt: String
}

struct AppServerWorkspaceMemoryNode: Decodable, Identifiable, Sendable {
    let id: String
    let sessionIds: [String]
    let type: String
    let title: String
    let summary: String
    let status: String
    let confidence: Double
    let tags: [String]
    let createdAt: String
    let updatedAt: String
    let revision: Int
    let duplicateMemories: [AppServerWorkspaceMemoryDuplicate]?
}

struct AppServerWorkspaceMemoryDuplicate: Decodable, Identifiable, Sendable {
    let id: String
    let type: String
    let title: String
    let status: String
    let revision: Int
    let markedAt: String
}

struct AppServerMemoryNotificationFeed: Decodable, Sendable {
    let schemaVersion: Int
    let workspaceId: String
    let profile: Profile
    let nodes: [AppServerMemoryNotificationNode]

    struct Profile: Decodable, Sendable {
        let id: String
        let version: String
        let hash: String
    }
}

struct AppServerMemoryNotificationNode: Decodable, Identifiable, Sendable {
    let id: String
    let kind: String
    let sessionIds: [String]
    let type: String
    let typeName: String
    let title: String
    let summary: String
    let status: String
    let heat: String
    let rating: String?
    let createdAt: String
    let updatedAt: String
    let revision: Int

    var notificationCheckpoint: String {
        "\(id)|\(revision)|\(heat)|\(rating ?? "")"
    }

    var isEligibleForIOSNotification: Bool {
        guard kind == "claim",
              rating == "medium" || rating == "high" else {
            return false
        }
        return status == "observed" || status == "reproduced" || status == "verified"
    }
}

struct AppServerWorkspaceSession: Decodable, Identifiable, Sendable {
    let id: String
    let workspaceId: String
    let status: String
    let title: String
    let prompt: String
    let startedAt: String
    let updatedAt: String
}

enum AppServerResearchProvider: String, CaseIterable, Encodable, Identifiable, Sendable {
    case openAI = "openai-codex"
    case anthropic
    case xAI = "xai"
    case zAI = "zai"
    case openRouter = "openrouter"

    var id: String { rawValue }

    var displayName: String {
        switch self {
        case .openAI: "OpenAI"
        case .anthropic: "Anthropic"
        case .xAI: "xAI"
        case .zAI: "Z.ai"
        case .openRouter: "OpenRouter"
        }
    }
}

enum AppServerReasoningEffort: String, CaseIterable, Encodable, Identifiable, Sendable {
    case minimal
    case low
    case medium
    case high
    case xhigh
    case max

    var id: String { rawValue }

    var displayName: String {
        switch self {
        case .xhigh: "Extra High"
        default: rawValue.capitalized
        }
    }
}

enum AppServerSubagentMode: String, CaseIterable, Encodable, Identifiable, Sendable {
    case simple
    case advanced

    var id: String { rawValue }
    var displayName: String { rawValue.capitalized }
}

enum AppServerShellSafetyMode: String, CaseIterable, Encodable, Identifiable, Sendable {
    case manualApproval = "manual_approval"
    case autoReview = "auto_review"
    case danger

    var id: String { rawValue }

    var displayName: String {
        switch self {
        case .manualApproval: "Manual Approval"
        case .autoReview: "Auto Review"
        case .danger: "Danger"
        }
    }
}

struct AppServerSessionCollaborator: Encodable, Equatable, Identifiable, Sendable {
    let id: UUID
    var provider: AppServerResearchProvider
    var model: String
    var reasoningEffort: AppServerReasoningEffort

    enum CodingKeys: String, CodingKey {
        case provider
        case model
        case reasoningEffort
        case enabled
    }

    init(
        id: UUID = UUID(),
        provider: AppServerResearchProvider = .openAI,
        model: String = "",
        reasoningEffort: AppServerReasoningEffort = .high
    ) {
        self.id = id
        self.provider = provider
        self.model = model
        self.reasoningEffort = reasoningEffort
    }

    func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encode(provider.rawValue, forKey: .provider)
        try container.encode(model.trimmingCharacters(in: .whitespacesAndNewlines), forKey: .model)
        try container.encode(reasoningEffort.rawValue, forKey: .reasoningEffort)
        try container.encode(true, forKey: .enabled)
    }
}

struct AppServerSessionLaunchConfiguration: Sendable {
    var leadProvider: AppServerResearchProvider?
    var leadModel: String
    var collaborators: [AppServerSessionCollaborator]
    var subagentMode: AppServerSubagentMode
    var shellSafetyMode: AppServerShellSafetyMode
    var goalEnabled: Bool
    var goalObjective: String?

    static let defaultValue = AppServerSessionLaunchConfiguration(
        leadProvider: nil,
        leadModel: "",
        collaborators: [],
        subagentMode: .simple,
        shellSafetyMode: .autoReview,
        goalEnabled: false,
        goalObjective: nil
    )
}

struct AppServerSessionLaunchRequest: Encodable, Sendable {
    struct Provider: Encodable, Sendable {
        let id: String?
        let model: String?
    }

    struct Collaboration: Encodable, Sendable {
        let mode = "always"
        let subagentMode: String
        let intensity = "balanced"
        let providers: [AppServerSessionCollaborator]
        let independentFirstPass = false
        let peerChallengeRounds = 0
        let maxConcurrentRooms = 2
        let maxMembersPerRoom = 3
    }

    struct Goal: Encodable, Sendable {
        let objective: String
    }

    struct Launch: Encodable, Sendable {
        let workspaceId: String
        let promptMarkdown: String
        let researchProfileId: String
        let provider: Provider?
        let shellSafetyMode: String
        let collaboration: Collaboration?
        let goal: Goal?
        let generateTitle: Bool
    }

    let launchVersion: Int
    let launch: Launch

    init(
        workspace: AppServerWorkspace,
        promptMarkdown: String,
        configuration: AppServerSessionLaunchConfiguration = .defaultValue
    ) {
        let leadModel = configuration.leadModel.trimmingCharacters(in: .whitespacesAndNewlines)
        let provider = configuration.leadProvider == nil && leadModel.isEmpty
            ? nil
            : Provider(
                id: configuration.leadProvider?.rawValue,
                model: leadModel.isEmpty ? nil : leadModel
            )
        let collaboration = configuration.collaborators.isEmpty
            ? nil
            : Collaboration(
                subagentMode: configuration.subagentMode.rawValue,
                providers: configuration.collaborators
            )
        let explicitGoal = configuration.goalObjective?
            .trimmingCharacters(in: .whitespacesAndNewlines)
        let goal = configuration.goalEnabled
            ? Goal(objective: explicitGoal.flatMap { $0.isEmpty ? nil : $0 } ?? promptMarkdown)
            : nil
        launchVersion = BealeAppServerContract.sessionLaunchVersion
        launch = Launch(
            workspaceId: workspace.workspaceId,
            promptMarkdown: promptMarkdown,
            researchProfileId: workspace.researchProfileId,
            provider: provider,
            shellSafetyMode: configuration.shellSafetyMode.rawValue,
            collaboration: collaboration,
            goal: goal,
            generateTitle: true
        )
    }
}

struct AppServerResearchPromptSuggestion: Decodable, Identifiable, Sendable {
    let title: String
    let promptMarkdown: String
    let rationale: String?

    var id: String { "\(title)\u{0}\(promptMarkdown)" }
}

struct AppServerGeneratedResearchSuggestions: Decodable, Sendable {
    let phase: String
    let suggestions: [String]
    let promptSuggestions: [AppServerResearchPromptSuggestion]?

    var fillableSuggestions: [AppServerResearchPromptSuggestion] {
        if let promptSuggestions, !promptSuggestions.isEmpty { return promptSuggestions }
        return suggestions.map {
            AppServerResearchPromptSuggestion(title: $0, promptMarkdown: $0, rationale: nil)
        }
    }
}

struct AppServerExpandedResearchPrompt: Decodable, Sendable {
    let phase: String
    let promptMarkdown: String
}

struct AppServerOperationResponse<Result: Decodable & Sendable>: Decodable, Sendable {
    let controlVersion: Int
    let result: Result
}

struct AppServerResearchSuggestionOperationInput: Encodable, Sendable {
    let workspaceId: String
    let refresh: Bool
}

struct AppServerHistoryDuplicateOperationInput: Encodable, Sendable {
    let workspaceId: String
    let type: String
    let id: String
    let parentId: String?
    let expectedRevision: Int
}

struct AppServerClaimMutationResult: Decodable, Sendable {
    let id: String
    let revision: Int
}

struct AppServerResearchPromptExpansionOperationInput: Encodable, Sendable {
    struct Provider: Encodable, Sendable {
        let id: String?
        let model: String?
    }

    let workspaceId: String
    let promptMarkdown: String
    let phase: String?
    let provider: Provider?

    init(
        workspaceId: String,
        promptMarkdown: String,
        phase: String?,
        configuration: AppServerSessionLaunchConfiguration
    ) {
        let model = configuration.leadModel.trimmingCharacters(in: .whitespacesAndNewlines)
        self.workspaceId = workspaceId
        self.promptMarkdown = promptMarkdown
        self.phase = phase
        provider = configuration.leadProvider == nil && model.isEmpty
            ? nil
            : Provider(
                id: configuration.leadProvider?.rawValue,
                model: model.isEmpty ? nil : model
            )
    }
}

struct AppServerOperationRequest<Input: Encodable & Sendable>: Encodable, Sendable {
    let operation: String
    let input: Input
}

struct AppServerCanonicalSessionEvents: Decodable, Sendable {
    let controlVersion: Int
    let workspace: AppServerWorkspace
    let result: AppServerSessionEventPage
}

struct AppServerCanonicalSessionCollaboration: Decodable, Sendable {
    let controlVersion: Int
    let workspace: AppServerWorkspace
    let result: AppServerSessionCollaborationState
}

struct AppServerSessionCollaborationState: Decodable, Sendable {
    let sessionId: String
    let revision: Int
    let rooms: [AppServerCollaborationEvent]
    let members: [AppServerCollaborationEvent]
    let messages: [AppServerCollaborationEvent]
    let subagents: [AppServerCollaborationEvent]
}

struct AppServerCollaborationEvent: Decodable, Identifiable, Sendable {
    struct Payload: Decodable, Sendable {
        let fields: [String: AppServerJSONValue]

        init(from decoder: Decoder) throws {
            fields = try decoder.singleValueContainer().decode([String: AppServerJSONValue].self)
        }

        func string(_ key: String) -> String? {
            if let value = fields[key]?.stringValue?.trimmingCharacters(in: .whitespacesAndNewlines),
               !value.isEmpty {
                return value
            }
            guard let nested = fields["payload"]?.objectValue,
                  let value = nested[key]?.stringValue?.trimmingCharacters(in: .whitespacesAndNewlines),
                  !value.isEmpty else {
                return nil
            }
            return value
        }
    }

    let id: String
    let kind: String
    let timestamp: String
    let summary: String
    let payload: Payload
    let agentId: String?
    let agentPath: String?
    let parentAgentId: String?
}

struct AppServerSubagentSummary: Identifiable, Equatable, Sendable {
    let id: String
    let agentId: String?
    let path: String
    let name: String
    let provider: String?
    let model: String?
    let channelName: String?
    let status: String
    let latestMessage: String
    let createdAt: String
    let lastActiveAt: String

    var isActive: Bool { status == "pending" || status == "running" }
}

enum AppServerSubagentProjection {
    static func summaries(
        from events: [AppServerCollaborationEvent],
        sessionStatus: String
    ) -> [AppServerSubagentSummary] {
        var summaries: [String: AppServerSubagentSummary] = [:]
        for event in events {
            guard let path = event.payload.string("agentPath") ?? event.agentPath,
                  path != "/root" else {
                continue
            }
            let action = event.payload.string("action")
            let prior = summaries[path]
            let eventTime = event.payload.string("timestamp") ?? event.timestamp
            let projectedStatus = status(
                explicit: event.payload.string("status"),
                action: action,
                fallback: prior?.status ?? "running"
            )
            let message = ["spawned", "message", "followup", "interrupted", "completed", "errored"]
                .contains(action ?? "")
                ? event.payload.string("message")
                : nil
            summaries[path] = AppServerSubagentSummary(
                id: path,
                agentId: event.payload.string("agentId") ?? event.agentId ?? prior?.agentId,
                path: path,
                name: path.split(separator: "/").last.map(String.init) ?? path,
                provider: event.payload.string("provider") ?? prior?.provider,
                model: event.payload.string("model") ?? prior?.model,
                channelName: event.payload.string("channelName")
                    ?? event.payload.string("channel_name")
                    ?? prior?.channelName,
                status: projectedStatus,
                latestMessage: message ?? prior?.latestMessage ?? "",
                createdAt: earlier(prior?.createdAt, eventTime),
                lastActiveAt: later(prior?.lastActiveAt, eventTime)
            )
        }
        return summaries.values.map { summary in
            guard summary.isActive,
                  !["active", "paused"].contains(sessionStatus) else {
                return summary
            }
            return AppServerSubagentSummary(
                id: summary.id,
                agentId: summary.agentId,
                path: summary.path,
                name: summary.name,
                provider: summary.provider,
                model: summary.model,
                channelName: summary.channelName,
                status: "interrupted",
                latestMessage: summary.latestMessage,
                createdAt: summary.createdAt,
                lastActiveAt: summary.lastActiveAt
            )
        }.sorted {
            if $0.isActive != $1.isActive { return $0.isActive }
            if $0.createdAt != $1.createdAt { return $0.createdAt > $1.createdAt }
            return $0.path < $1.path
        }
    }

    private static func status(explicit: String?, action: String?, fallback: String) -> String {
        if let explicit,
           ["pending", "running", "completed", "interrupted", "errored"].contains(explicit) {
            return explicit
        }
        return switch action {
        case "spawned", "followup": "running"
        case "completed": "completed"
        case "interrupted": "interrupted"
        case "errored": "errored"
        default: fallback
        }
    }

    private static func earlier(_ left: String?, _ right: String) -> String {
        guard let left else { return right }
        return left <= right ? left : right
    }

    private static func later(_ left: String?, _ right: String) -> String {
        guard let left else { return right }
        return left >= right ? left : right
    }
}

struct AppServerSessionEventPage: Decodable, Sendable {
    let sessionId: String
    let stream: String
    let events: [AppServerTranscriptEvent]
    let eventOffset: Int
    let nextAfterEventId: String?
    let hasEarlier: Bool
    let hasMore: Bool
}

indirect enum AppServerJSONValue: Decodable, Equatable, Sendable {
    case object([String: AppServerJSONValue])
    case array([AppServerJSONValue])
    case string(String)
    case number(Double)
    case boolean(Bool)
    case null

    init(from decoder: Decoder) throws {
        let container = try decoder.singleValueContainer()
        if container.decodeNil() {
            self = .null
        } else if let value = try? container.decode(Bool.self) {
            self = .boolean(value)
        } else if let value = try? container.decode(Double.self) {
            self = .number(value)
        } else if let value = try? container.decode(String.self) {
            self = .string(value)
        } else if let value = try? container.decode([String: AppServerJSONValue].self) {
            self = .object(value)
        } else if let value = try? container.decode([AppServerJSONValue].self) {
            self = .array(value)
        } else {
            throw DecodingError.dataCorruptedError(in: container, debugDescription: "Unsupported JSON value.")
        }
    }

    var stringValue: String? {
        guard case .string(let value) = self else { return nil }
        return value
    }

    var objectValue: [String: AppServerJSONValue]? {
        guard case .object(let value) = self else { return nil }
        return value
    }

    var stringArrayValue: [String]? {
        guard case .array(let values) = self else { return nil }
        return values.compactMap(\.stringValue)
    }
}

struct AppServerCanonicalSessionApprovalEvents: Decodable, Sendable {
    let controlVersion: Int
    let workspace: AppServerWorkspace
    let result: AppServerSessionApprovalEventPage
}

struct AppServerSessionApprovalEventPage: Decodable, Sendable {
    let sessionId: String
    let stream: String
    let events: [AppServerApprovalEvent]
}

struct AppServerApprovalEvent: Decodable, Identifiable, Sendable {
    struct Payload: Decodable, Sendable {
        let fields: [String: AppServerJSONValue]

        init(from decoder: Decoder) throws {
            fields = try decoder.singleValueContainer().decode([String: AppServerJSONValue].self)
        }
    }

    let id: String
    let kind: String
    let timestamp: String
    let payload: Payload
}

struct AppServerPendingApproval: Identifiable, Equatable, Sendable {
    let id: String
    let requestKind: String
    let requestedAction: [String: AppServerJSONValue]
    let createdAt: String

    var isComputerUse: Bool { requestKind == "computer_use" }

    var approvalKind: String? { requestedAction["approvalKind"]?.stringValue }

    var permissionMode: String? { requestedAction["permissionMode"]?.stringValue }

    var reviewReason: String? { requestedAction["reviewReason"]?.stringValue }

    var targetBinary: String? {
        if let target = requestedAction["targetBinary"]?.stringValue, !target.isEmpty { return target }
        return requestedAction["arguments"]?.objectValue?["process"]?.stringValue
    }

    var toolName: String? { requestedAction["toolName"]?.stringValue }

    var commandLine: String? {
        guard let command = requestedAction["command"]?.objectValue,
              let utility = command["utility"]?.stringValue else {
            return nil
        }
        return ([utility] + (command["args"]?.stringArrayValue ?? [])).joined(separator: " ")
    }
}

enum AppServerApprovalProjection {
    static func pendingApprovals(_ events: [AppServerApprovalEvent]) -> [AppServerPendingApproval] {
        var pending: [String: AppServerPendingApproval] = [:]
        // Canonical session events are append ordered. Preserve that order so
        // revisions whose records retain their original createdAt still win.
        for event in events {
            let payload = event.payload.fields
            if event.kind == "beale.approval", let record = payload["record"]?.objectValue {
                applyPersistedRecord(record, timestamp: event.timestamp, to: &pending)
                continue
            }
            guard event.kind == "agent.event",
                  let eventType = payload["type"]?.stringValue,
                  let requestId = payload["approvalRequestId"]?.stringValue,
                  !requestId.isEmpty else {
                continue
            }
            switch eventType {
            case "shell_authorization_requested":
                pending[requestId] = AppServerPendingApproval(
                    id: requestId,
                    requestKind: "shell_command",
                    requestedAction: payload,
                    createdAt: event.timestamp
                )
            case "tool_authorization_requested":
                pending[requestId] = AppServerPendingApproval(
                    id: requestId,
                    requestKind: "computer_use",
                    requestedAction: payload,
                    createdAt: event.timestamp
                )
            case "shell_authorization_resolved", "tool_authorization_resolved":
                pending[requestId] = nil
            default:
                break
            }
        }
        return pending.values.sorted {
            $0.createdAt == $1.createdAt ? $0.id < $1.id : $0.createdAt < $1.createdAt
        }
    }

    private static func applyPersistedRecord(
        _ record: [String: AppServerJSONValue],
        timestamp: String,
        to pending: inout [String: AppServerPendingApproval]
    ) {
        let requestKind = record["requestKind"]?.stringValue ?? "shell_command"
        guard requestKind == "shell_command" || requestKind == "computer_use",
              let action = record["requestedAction"]?.objectValue,
              let requestId = action["approvalRequestId"]?.stringValue,
              !requestId.isEmpty else {
            return
        }
        if record["decision"]?.stringValue == "pending" {
            pending[requestId] = AppServerPendingApproval(
                id: requestId,
                requestKind: requestKind,
                requestedAction: action,
                createdAt: record["createdAt"]?.stringValue ?? timestamp
            )
        } else {
            pending[requestId] = nil
        }
    }

}

struct AppServerTranscriptEvent: Decodable, Identifiable, Sendable {
    struct Payload: Decodable, Sendable {
        let record: AppServerTranscriptMessage?
    }

    let id: String
    let kind: String
    let timestamp: String
    let summary: String
    let payload: Payload
}

struct AppServerTranscriptMessage: Decodable, Identifiable, Sendable {
    struct Metadata: Decodable, Sendable {
        let agentPath: String?
        let responseId: String?
        let itemId: String?
        let toolName: String?
        let toolCount: Int?
        let toolPluralTemplate: String?

        init(
            agentPath: String? = nil,
            responseId: String? = nil,
            itemId: String? = nil,
            toolName: String? = nil,
            toolCount: Int? = nil,
            toolPluralTemplate: String? = nil
        ) {
            self.agentPath = agentPath
            self.responseId = responseId
            self.itemId = itemId
            self.toolName = toolName
            self.toolCount = toolCount
            self.toolPluralTemplate = toolPluralTemplate
        }
    }

    let id: String
    let runId: String?
    let attemptId: String?
    let traceEventId: String?
    let role: String
    let phase: String?
    let contentMarkdown: String
    let source: String
    let metadata: Metadata?
    let createdAt: String
}

enum AppServerTranscriptProjection {
    static func rootMessages(_ messages: [AppServerTranscriptMessage]) -> [AppServerTranscriptMessage] {
        project(messages.filter(isRootMessage))
    }

    static func subagentMessages(
        _ messages: [AppServerTranscriptMessage],
        path: String
    ) -> [AppServerTranscriptMessage] {
        project(messages.filter {
            $0.metadata?.agentPath?.trimmingCharacters(in: .whitespacesAndNewlines) == path
        })
    }

    private static func project(_ messages: [AppServerTranscriptMessage]) -> [AppServerTranscriptMessage] {
        let nativeCommentaryKeys = Set(messages.compactMap(nativeCommentaryKey))
        var latestMessageIndexByKey: [String: Int] = [:]
        var latestReasoningIndexByKey: [String: Int] = [:]

        for (index, message) in messages.enumerated() {
            if let key = duplicateMessageKey(message) {
                latestMessageIndexByKey[key] = index
            }
            if let key = reasoningSnapshotKey(message) {
                latestReasoningIndexByKey[key] = index
            }
        }

        let projected: [AppServerTranscriptMessage] = messages.enumerated().compactMap { index, message in
            if let key = duplicateMessageKey(message), latestMessageIndexByKey[key] != index {
                return nil
            }
            if let key = reasoningSnapshotKey(message), latestReasoningIndexByKey[key] != index {
                return nil
            }
            if message.source == "openai_reasoning_summary",
               let key = responseKey(message),
               nativeCommentaryKeys.contains(key) {
                return nil
            }
            return message
        }
        return coalesceToolSummaries(projected)
    }

    private static func duplicateMessageKey(_ message: AppServerTranscriptMessage) -> String? {
        let agentPath = message.metadata?.agentPath?.trimmingCharacters(in: .whitespacesAndNewlines)
        let normalizedAgentPath = agentPath.flatMap { $0.isEmpty ? nil : $0 } ?? "/root"
        let responseId = message.metadata?.responseId?.trimmingCharacters(in: .whitespacesAndNewlines)
        let itemId = message.metadata?.itemId?.trimmingCharacters(in: .whitespacesAndNewlines)
        if responseId?.isEmpty == false || itemId?.isEmpty == false {
            return [
                "model",
                message.role,
                message.source,
                normalizedAgentPath,
                responseId ?? "",
                itemId ?? ""
            ].joined(separator: "\u{0}")
        }
        if let traceEventId = message.traceEventId?.trimmingCharacters(in: .whitespacesAndNewlines),
           !traceEventId.isEmpty {
            return ["trace", message.role, message.source, traceEventId].joined(separator: "\u{0}")
        }
        guard !message.createdAt.isEmpty, !message.contentMarkdown.isEmpty else { return nil }
        return [
            "exact",
            message.role,
            message.phase ?? "",
            message.source,
            normalizedAgentPath,
            message.metadata?.toolName ?? "",
            message.createdAt,
            message.contentMarkdown
        ].joined(separator: "\u{0}")
    }

    private static func isRootMessage(_ message: AppServerTranscriptMessage) -> Bool {
        let agentPath = message.metadata?.agentPath?.trimmingCharacters(in: .whitespacesAndNewlines)
        return agentPath == nil || agentPath?.isEmpty == true || agentPath == "/root"
    }

    private static func nativeCommentaryKey(_ message: AppServerTranscriptMessage) -> String? {
        guard message.source == "app_server_commentary" || (
            message.role == "assistant"
                && message.phase == "commentary"
                && message.source != "openai_reasoning_summary"
        ) else {
            return nil
        }
        return responseKey(message)
    }

    private static func responseKey(_ message: AppServerTranscriptMessage) -> String? {
        guard let responseId = message.metadata?.responseId?.trimmingCharacters(in: .whitespacesAndNewlines),
              !responseId.isEmpty else {
            return nil
        }
        return [
            message.attemptId ?? "",
            message.metadata?.agentPath ?? "/root",
            responseId
        ].joined(separator: "\u{0}")
    }

    private static func reasoningSnapshotKey(_ message: AppServerTranscriptMessage) -> String? {
        guard message.source == "openai_reasoning_summary",
              let responseId = message.metadata?.responseId?.trimmingCharacters(in: .whitespacesAndNewlines),
              !responseId.isEmpty,
              let itemId = message.metadata?.itemId?.trimmingCharacters(in: .whitespacesAndNewlines),
              !itemId.isEmpty else {
            return nil
        }
        return [
            message.attemptId ?? "",
            message.metadata?.agentPath ?? "/root",
            responseId,
            itemId
        ].joined(separator: "\u{0}")
    }

    private static func coalesceToolSummaries(
        _ messages: [AppServerTranscriptMessage]
    ) -> [AppServerTranscriptMessage] {
        var coalesced: [AppServerTranscriptMessage] = []
        for message in messages {
            guard message.source == "app_server_tool_summary",
                  let toolName = message.metadata?.toolName,
                  let previous = coalesced.last,
                  previous.source == message.source,
                  previous.metadata?.toolName == toolName else {
                coalesced.append(message)
                continue
            }
            let count = (previous.metadata?.toolCount ?? 1) + (message.metadata?.toolCount ?? 1)
            let template = message.metadata?.toolPluralTemplate
                ?? previous.metadata?.toolPluralTemplate
                ?? "Used {count} tools"
            let metadata = AppServerTranscriptMessage.Metadata(
                agentPath: message.metadata?.agentPath,
                toolName: toolName,
                toolCount: count,
                toolPluralTemplate: template
            )
            coalesced[coalesced.count - 1] = AppServerTranscriptMessage(
                id: message.id,
                runId: message.runId,
                attemptId: message.attemptId,
                traceEventId: nil,
                role: message.role,
                phase: message.phase,
                contentMarkdown: template.replacingOccurrences(of: "{count}", with: String(count)),
                source: message.source,
                metadata: metadata,
                createdAt: message.createdAt
            )
        }
        return coalesced
    }

}

struct AppServerSessionCatalog: Decodable, Sendable {
    let controlVersion: Int
    let sessions: [AppServerSession]
}

struct AppServerSessionStopResult: Decodable, Sendable {
    let controlVersion: Int
    let stopped: Bool
    let sessionId: String

    func validate(sessionId expectedSessionId: String) throws {
        guard controlVersion == BealeAppServerContract.controlVersion,
              sessionId == expectedSessionId else {
            throw AppServerClientError.incompatible("The session stop response did not match this request.")
        }
    }
}

struct AppServerSession: Decodable, Identifiable, Sendable {
    let sessionId: String
    let state: String
    let startedAt: String
    let endedAt: String?
    let exitCode: Int?
    let clientConnected: Bool
    let diagnostic: String?
    let replay: AppServerReplayStatus

    var id: String { sessionId }

    var isActive: Bool {
        state == "starting" || state == "running"
    }
}

struct AppServerReplayStatus: Decodable, Sendable {
    let bufferedFrames: Int
    let bufferedBytes: Int
    let droppedFrames: Int
}

struct AppServerSessionAttachment: Decodable, Sendable {
    let controlVersion: Int
    let session: AppServerSession
    let transport: AppServerSessionTransport
}

struct AppServerSessionStart: Decodable, Sendable {
    let controlVersion: Int
    let session: AppServerSession
    let attemptId: String
    let transport: AppServerSessionTransport

    func validateCompatibility() throws {
        guard controlVersion == BealeAppServerContract.controlVersion,
              !attemptId.isEmpty,
              transport.path == "/v1/sessions/\(session.sessionId)/transport",
              transport.protocolVersion == BealeAppServerContract.appServerProtocolVersion,
              transport.authentication == "bearer",
              !transport.token.isEmpty,
              transport.reconnect == "replay" else {
            throw AppServerClientError.incompatible("The session start response did not match this request.")
        }
    }
}

struct AppServerSessionTransport: Decodable, Sendable {
    let path: String
    let protocolVersion: Int
    let authentication: String
    let token: String
    let reconnect: String
}

struct AppServerErrorResponse: Decodable, Sendable {
    struct Detail: Decodable, Sendable {
        let code: String
        let message: String
        let retryable: Bool
    }

    let controlVersion: Int
    let error: Detail
}
