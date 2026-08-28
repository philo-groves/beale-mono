import XCTest
@testable import Beale

private final class TestOperatorTokenStore: OperatorTokenStore, @unchecked Sendable {
    private let lock = NSLock()
    private var tokens: [String: String] = [:]
    private var legacyToken = ""

    init(legacyToken: String = "") {
        self.legacyToken = legacyToken
    }

    func loadOperatorToken(connectionID: String) -> String {
        lock.lock()
        defer { lock.unlock() }
        return tokens[connectionID] ?? ""
    }

    func saveOperatorToken(_ token: String, connectionID: String) throws {
        lock.lock()
        defer { lock.unlock() }
        tokens[connectionID] = token
    }

    func deleteOperatorToken(connectionID: String) {
        lock.lock()
        defer { lock.unlock() }
        tokens[connectionID] = nil
    }

    func loadLegacyOperatorToken() -> String {
        lock.lock()
        defer { lock.unlock() }
        return legacyToken
    }

    func deleteLegacyOperatorToken() {
        lock.lock()
        defer { lock.unlock() }
        legacyToken = ""
    }
}

final class AppServerContractTests: XCTestCase {
    @MainActor
    func testPersistsMultipleIndependentConnections() async {
        let (defaults, suiteName) = makeIsolatedDefaults()
        defer { defaults.removePersistentDomain(forName: suiteName) }
        let tokenStore = TestOperatorTokenStore()
        let model = AppModel(
            defaults: defaults,
            keychain: tokenStore,
            snapshotLoader: successfulSnapshot
        )

        await model.configureAndConnect(
            serverURL: "https://first.example.ts.net",
            operatorToken: "first-token"
        )
        await model.configureAndConnect(
            serverURL: "https://second.example.ts.net",
            operatorToken: "second-token"
        )

        XCTAssertEqual(
            Set(model.connections.map(\.serverURL)),
            ["https://first.example.ts.net", "https://second.example.ts.net"]
        )
        let first = model.connections.first { $0.serverURL == "https://first.example.ts.net" }
        let second = model.connections.first { $0.serverURL == "https://second.example.ts.net" }
        XCTAssertEqual(first.map(model.operatorToken(for:)), "first-token")
        XCTAssertEqual(second.map(model.operatorToken(for:)), "second-token")

        let reloaded = AppModel(
            defaults: defaults,
            keychain: tokenStore,
            snapshotLoader: successfulSnapshot
        )
        XCTAssertEqual(reloaded.connections.count, 2)
        XCTAssertEqual(reloaded.serverURL, "https://second.example.ts.net")
        XCTAssertEqual(reloaded.operatorToken, "second-token")
    }

    @MainActor
    func testEditingOneConnectionDoesNotReplaceTheOthers() async throws {
        let (defaults, suiteName) = makeIsolatedDefaults()
        defer { defaults.removePersistentDomain(forName: suiteName) }
        let tokenStore = TestOperatorTokenStore()
        let model = AppModel(
            defaults: defaults,
            keychain: tokenStore,
            snapshotLoader: successfulSnapshot
        )

        await model.configureAndConnect(
            serverURL: "https://first.example.ts.net",
            operatorToken: "first-token"
        )
        let firstID = try XCTUnwrap(model.connections.first?.id)
        await model.configureAndConnect(
            serverURL: "https://second.example.ts.net",
            operatorToken: "second-token"
        )
        await model.configureAndConnect(
            serverURL: "https://renamed.example.ts.net",
            operatorToken: "updated-token",
            replacing: firstID
        )

        XCTAssertEqual(model.connections.count, 2)
        XCTAssertEqual(model.connections.first(where: { $0.id == firstID })?.serverURL, "https://renamed.example.ts.net")
        XCTAssertTrue(model.connections.contains { $0.serverURL == "https://second.example.ts.net" })
    }

    @MainActor
    func testForgettingInactiveConnectionKeepsActiveConnection() async throws {
        let (defaults, suiteName) = makeIsolatedDefaults()
        defer { defaults.removePersistentDomain(forName: suiteName) }
        let tokenStore = TestOperatorTokenStore()
        let model = AppModel(
            defaults: defaults,
            keychain: tokenStore,
            snapshotLoader: successfulSnapshot
        )

        await model.configureAndConnect(
            serverURL: "https://first.example.ts.net",
            operatorToken: "first-token"
        )
        await model.configureAndConnect(
            serverURL: "https://second.example.ts.net",
            operatorToken: "second-token"
        )
        let inactiveConnection = try XCTUnwrap(model.connections.first { !$0.isActive })
        model.forgetConnection(inactiveConnection)

        XCTAssertEqual(model.connections.count, 1)
        XCTAssertEqual(model.connections.first?.serverURL, "https://second.example.ts.net")
        XCTAssertTrue(model.isConnected)
        XCTAssertEqual(tokenStore.loadOperatorToken(connectionID: inactiveConnection.id), "")
    }

    @MainActor
    func testMigratesLegacySingleConnectionIntoCatalog() {
        let (defaults, suiteName) = makeIsolatedDefaults()
        defer { defaults.removePersistentDomain(forName: suiteName) }
        defaults.set("https://legacy.example.ts.net", forKey: "BealeServerURL")
        let tokenStore = TestOperatorTokenStore(legacyToken: "legacy-token")

        let model = AppModel(
            defaults: defaults,
            keychain: tokenStore,
            snapshotLoader: successfulSnapshot
        )

        XCTAssertEqual(model.connections.map(\.serverURL), ["https://legacy.example.ts.net"])
        XCTAssertEqual(model.operatorToken, "legacy-token")
        XCTAssertNil(defaults.string(forKey: "BealeServerURL"))
    }

    @MainActor
    func testConnectionFailureHintsWhenTailscaleHostnameCannotBeResolved() async {
        let (defaults, suiteName) = makeIsolatedDefaults()
        defer { defaults.removePersistentDomain(forName: suiteName) }
        let model = AppModel(
            defaults: defaults,
            keychain: TestOperatorTokenStore(),
            snapshotLoader: { _, _ in throw URLError(.cannotFindHost) }
        )

        await model.configureAndConnect(
            serverURL: "https://unavailable.example.ts.net",
            operatorToken: "test-token"
        )

        guard case .failed(let message) = model.connectionState else {
            return XCTFail("Expected the connection to fail.")
        }
        XCTAssertTrue(message.contains("Tailscale is connected on this iPhone"))
        XCTAssertTrue(message.contains("same tailnet as the Mac"))
    }

    func testAppForcesDesktopMatchingDarkAppearance() {
        XCTAssertEqual(Bundle.main.object(forInfoDictionaryKey: "UIUserInterfaceStyle") as? String, "Dark")
    }

    func testAppDeclaresCameraPurposeForPairingScanner() {
        XCTAssertNotNil(Bundle.main.object(forInfoDictionaryKey: "NSCameraUsageDescription") as? String)
    }

    private func makeIsolatedDefaults() -> (UserDefaults, String) {
        let suiteName = "AppServerContractTests.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suiteName)!
        defaults.removePersistentDomain(forName: suiteName)
        return (defaults, suiteName)
    }

    private var successfulSnapshot: @Sendable (AppServerEndpoint, String) async throws -> AppServerSnapshot {
        { _, _ in
            AppServerSnapshot(
                health: AppServerHealth(
                    ok: true,
                    controlVersion: BealeAppServerContract.controlVersion,
                    contractTimestamp: "test",
                    capabilities: Array(BealeAppServerContract.requiredCapabilities)
                ),
                providerCatalog: AppServerProviderCatalog(
                    controlVersion: BealeAppServerContract.controlVersion,
                    defaultProviderId: "openai-codex",
                    providers: []
                ),
                workspaces: [],
                sessions: []
            )
        }
    }

    func testDecodesVersionedPairingQRCode() throws {
        let payload = try AppServerPairingPayload(
            scannedValue: "beale://connect?v=1&url=https%3A%2F%2Fbeale.example.ts.net&token=operator_token-123"
        )
        XCTAssertEqual(payload.serverURL, "https://beale.example.ts.net")
        XCTAssertEqual(payload.operatorToken, "operator_token-123")
    }

    func testRejectsInvalidPairingQRCodes() {
        XCTAssertThrowsError(try AppServerPairingPayload(scannedValue: "https://beale.example.ts.net"))
        XCTAssertThrowsError(try AppServerPairingPayload(
            scannedValue: "beale://connect?v=2&url=https%3A%2F%2Fbeale.example.ts.net&token=token"
        ))
        XCTAssertThrowsError(try AppServerPairingPayload(
            scannedValue: "beale://connect?v=1&url=http%3A%2F%2Fbeale.example.ts.net&token=token"
        ))
    }

    func testConfiguredAppServerIntegration() async throws {
        let environment = ProcessInfo.processInfo.environment
        guard let rawURL = environment["BEALE_IOS_INTEGRATION_URL"],
              let token = environment["BEALE_IOS_INTEGRATION_TOKEN"] else {
            throw XCTSkip("Set the Beale iOS integration URL and token to exercise a live app-server.")
        }

        let snapshot = try await AppServerClient(
            endpoint: AppServerEndpoint(rawURL),
            operatorToken: token
        ).fetchSnapshot()
        XCTAssertTrue(snapshot.health.ok)
        XCTAssertFalse(snapshot.workspaces.isEmpty)
    }

    func testAcceptsCompatibleHealthResponse() throws {
        let data = try XCTUnwrap(
            """
            {
              "ok": true,
              "controlVersion": 1,
              "contractTimestamp": "2026-08-22T01:44:44.447Z",
              "capabilities": [
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
                "workspace.goal-suggestions.v1",
                "workspace.prompt-expansion.v1"
              ]
            }
            """.data(using: .utf8)
        )
        let health = try JSONDecoder().decode(AppServerHealth.self, from: data)
        XCTAssertNoThrow(try health.validateCompatibility())
    }

    func testRejectsMissingCapabilities() throws {
        let health = AppServerHealth(
            ok: true,
            controlVersion: 1,
            contractTimestamp: "test",
            capabilities: ["host.control.v1"]
        )
        XCTAssertThrowsError(try health.validateCompatibility())
    }

    func testDecodesConnectedProviderCatalogWithHostDefaults() throws {
        let data = try XCTUnwrap(
            """
            {
              "controlVersion": 1,
              "defaultProviderId": "openai-codex",
              "providers": [{
                "providerId": "openai-codex",
                "providerName": "OpenAI",
                "defaultLeadModel": "gpt-5.6-sol",
                "defaultSubagentModel": "gpt-5.6-luna",
                "defaultReasoningEffort": "high",
                "models": [{
                  "id": "gpt-5.6-sol",
                  "name": "GPT-5.6 Sol",
                  "reasoning": true,
                  "effortLevels": ["low", "medium", "high"]
                }]
              }]
            }
            """.data(using: .utf8)
        )
        let catalog = try JSONDecoder().decode(AppServerProviderCatalog.self, from: data)

        XCTAssertNoThrow(try catalog.validateCompatibility())
        XCTAssertEqual(catalog.defaultProviderId, "openai-codex")
        XCTAssertEqual(catalog.providers.first?.defaultLeadModel, "gpt-5.6-sol")
        XCTAssertEqual(catalog.providers.first?.defaultSubagentModel, "gpt-5.6-luna")
        XCTAssertEqual(catalog.providers.first?.models.first?.name, "GPT-5.6 Sol")
    }

    func testNormalizesSecureEndpoint() throws {
        let endpoint = try AppServerEndpoint("  https://beale.example.ts.net/  ")
        XCTAssertEqual(endpoint.baseURL.absoluteString, "https://beale.example.ts.net")
        XCTAssertEqual(endpoint.url(path: "/v1/workspaces").absoluteString, "https://beale.example.ts.net/v1/workspaces")
        XCTAssertEqual(
            endpoint.url(
                pathComponents: ["v1", "workspaces", "workspace-1", "sessions"],
                queryItems: [URLQueryItem(name: "limit", value: "200")]
            ).absoluteString,
            "https://beale.example.ts.net/v1/workspaces/workspace-1/sessions?limit=200"
        )
        XCTAssertEqual(
            endpoint.url(
                pathComponents: ["v1", "workspaces", "workspace-1", "sessions", "session-1", "events"],
                queryItems: [
                    URLQueryItem(name: "stream", value: "commentary"),
                    URLQueryItem(name: "afterEventId", value: "event-1")
                ]
            ).absoluteString,
            "https://beale.example.ts.net/v1/workspaces/workspace-1/sessions/session-1/events?stream=commentary&afterEventId=event-1"
        )
        XCTAssertEqual(
            try endpoint.webSocketURL(path: "/v1/sessions/session-1/transport").absoluteString,
            "wss://beale.example.ts.net/v1/sessions/session-1/transport"
        )
    }

    func testDecodesIndependentSessionAttachment() throws {
        let data = try XCTUnwrap(
            """
            {
              "controlVersion": 1,
              "session": {
                "sessionId": "session-1",
                "state": "running",
                "startedAt": "2026-08-22T00:00:00.000Z",
                "endedAt": null,
                "exitCode": null,
                "clientConnected": true,
                "diagnostic": null,
                "replay": {"bufferedFrames": 0, "bufferedBytes": 0, "droppedFrames": 0}
              },
              "transport": {
                "path": "/v1/sessions/session-1/transport",
                "protocolVersion": 1,
                "authentication": "bearer",
                "token": "mobile-token",
                "reconnect": "replay"
              }
            }
            """.data(using: .utf8)
        )
        let attachment = try JSONDecoder().decode(AppServerSessionAttachment.self, from: data)
        XCTAssertEqual(attachment.session.sessionId, "session-1")
        XCTAssertEqual(attachment.transport.token, "mobile-token")
    }

    func testValidatesSessionStopResponseIdentity() throws {
        let data = try XCTUnwrap(
            """
            {
              "controlVersion": 1,
              "stopped": true,
              "sessionId": "session-1"
            }
            """.data(using: .utf8)
        )
        let result = try JSONDecoder().decode(AppServerSessionStopResult.self, from: data)

        XCTAssertTrue(result.stopped)
        XCTAssertNoThrow(try result.validate(sessionId: "session-1"))
        XCTAssertThrowsError(try result.validate(sessionId: "session-2"))
    }

    func testEncodesPathFreeTypedSessionLaunch() throws {
        let workspace = AppServerWorkspace(
            id: "registry-1",
            workspaceId: "workspace-1",
            name: "Example",
            researchProfileId: "security-research",
            researchKitId: "general",
            runCount: 0,
            lastRunAt: nil,
            updatedAt: "2026-08-28T00:00:00.000Z"
        )
        let data = try JSONEncoder().encode(
            AppServerSessionLaunchRequest(workspace: workspace, promptMarkdown: "Review the authorization boundary.")
        )
        let object = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
        let launch = try XCTUnwrap(object["launch"] as? [String: Any])

        XCTAssertEqual(object["launchVersion"] as? Int, 2)
        XCTAssertEqual(launch["workspaceId"] as? String, "workspace-1")
        XCTAssertEqual(launch["promptMarkdown"] as? String, "Review the authorization boundary.")
        XCTAssertEqual(launch["researchProfileId"] as? String, "security-research")
        XCTAssertEqual(launch["generateTitle"] as? Bool, true)
        XCTAssertNil(launch["provider"])
        XCTAssertEqual(launch["shellSafetyMode"] as? String, "auto_review")
        XCTAssertNil(launch["collaboration"])
        XCTAssertFalse(String(data: data, encoding: .utf8)?.contains("workspacePath") ?? true)
    }

    func testEncodesConfiguredSessionModelsCollaborationAndSafety() throws {
        let workspace = AppServerWorkspace(
            id: "registry-1",
            workspaceId: "workspace-1",
            name: "Example",
            researchProfileId: "security-research",
            researchKitId: "general",
            runCount: 0,
            lastRunAt: nil,
            updatedAt: "2026-08-28T00:00:00.000Z"
        )
        let configuration = AppServerSessionLaunchConfiguration(
            leadProvider: .anthropic,
            leadModel: " claude-opus-5 ",
            collaborators: [
                AppServerSessionCollaborator(
                    provider: .openAI,
                    model: " gpt-5.6-sol ",
                    reasoningEffort: .xhigh
                ),
                AppServerSessionCollaborator(
                    provider: .xAI,
                    model: "grok-4.6",
                    reasoningEffort: .high
                )
            ],
            subagentMode: .advanced,
            shellSafetyMode: .manualApproval,
            goalEnabled: true,
            goalObjective: "Review the authorization boundary."
        )
        let data = try JSONEncoder().encode(
            AppServerSessionLaunchRequest(
                workspace: workspace,
                promptMarkdown: "Review the authorization boundary.",
                configuration: configuration
            )
        )
        let object = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
        let launch = try XCTUnwrap(object["launch"] as? [String: Any])
        let provider = try XCTUnwrap(launch["provider"] as? [String: Any])
        let collaboration = try XCTUnwrap(launch["collaboration"] as? [String: Any])
        let collaborators = try XCTUnwrap(collaboration["providers"] as? [[String: Any]])

        XCTAssertEqual(provider["id"] as? String, "anthropic")
        XCTAssertEqual(provider["model"] as? String, "claude-opus-5")
        XCTAssertEqual(launch["shellSafetyMode"] as? String, "manual_approval")
        XCTAssertEqual(
            (launch["goal"] as? [String: Any])?["objective"] as? String,
            "Review the authorization boundary."
        )
        XCTAssertEqual(collaboration["mode"] as? String, "always")
        XCTAssertEqual(collaboration["subagentMode"] as? String, "advanced")
        XCTAssertEqual(collaboration["intensity"] as? String, "balanced")
        XCTAssertEqual(collaboration["independentFirstPass"] as? Bool, false)
        XCTAssertEqual(collaboration["peerChallengeRounds"] as? Int, 0)
        XCTAssertEqual(collaboration["maxConcurrentRooms"] as? Int, 2)
        XCTAssertEqual(collaboration["maxMembersPerRoom"] as? Int, 3)
        XCTAssertEqual(collaborators.count, 2)
        XCTAssertEqual(collaborators[0]["provider"] as? String, "openai-codex")
        XCTAssertEqual(collaborators[0]["model"] as? String, "gpt-5.6-sol")
        XCTAssertEqual(collaborators[0]["reasoningEffort"] as? String, "xhigh")
        XCTAssertEqual(collaborators[0]["enabled"] as? Bool, true)
        XCTAssertNil(collaborators[0]["id"])
        XCTAssertEqual(collaborators[1]["provider"] as? String, "xai")
        XCTAssertEqual(collaborators[1]["model"] as? String, "grok-4.6")
    }

    func testDecodesCompatibleSessionStart() throws {
        let data = try XCTUnwrap(
            """
            {
              "controlVersion": 1,
              "session": {
                "sessionId": "session-new",
                "state": "running",
                "startedAt": "2026-08-28T00:00:00.000Z",
                "endedAt": null,
                "exitCode": null,
                "clientConnected": false,
                "diagnostic": null,
                "replay": {"bufferedFrames": 0, "bufferedBytes": 0, "droppedFrames": 0}
              },
              "attemptId": "attempt-new",
              "transport": {
                "path": "/v1/sessions/session-new/transport",
                "protocolVersion": 1,
                "authentication": "bearer",
                "token": "session-token",
                "reconnect": "replay"
              }
            }
            """.data(using: .utf8)
        )
        let result = try JSONDecoder().decode(AppServerSessionStart.self, from: data)

        XCTAssertNoThrow(try result.validateCompatibility())
        XCTAssertEqual(result.session.sessionId, "session-new")
        XCTAssertEqual(result.attemptId, "attempt-new")
    }

    func testRejectsInsecureTailnetEndpoint() {
        XCTAssertThrowsError(try AppServerEndpoint("http://beale.example.ts.net"))
    }

    func testDecodesWorkspaceCatalogWithoutHostPaths() throws {
        let data = try XCTUnwrap(
            """
            {
              "controlVersion": 1,
              "workspaces": [{
                "id": "registry-1",
                "workspaceId": "workspace-1",
                "name": "Example",
                "researchProfileId": "security-research",
                "researchKitId": "general",
                "runCount": 2,
                "lastRunAt": null,
                "updatedAt": "2026-08-22T00:00:00.000Z"
              }]
            }
            """.data(using: .utf8)
        )
        let result = try JSONDecoder().decode(AppServerWorkspaceList.self, from: data)
        XCTAssertEqual(result.workspaces.first?.workspaceId, "workspace-1")
        XCTAssertFalse(String(data: data, encoding: .utf8)?.contains("workspacePath") ?? true)
    }

    func testDecodesNamedCanonicalWorkspaceSessions() throws {
        let data = try XCTUnwrap(
            """
            {
              "controlVersion": 1,
              "workspace": {
                "id": "registry-1",
                "workspaceId": "workspace-1",
                "name": "Example",
                "researchProfileId": "security-research",
                "researchKitId": "general",
                "runCount": 2,
                "lastRunAt": null,
                "updatedAt": "2026-08-22T00:00:00.000Z"
              },
              "result": [{
                "schemaVersion": 4,
                "id": "session-1",
                "workspaceId": "workspace-1",
                "status": "completed",
                "title": "Review authentication boundary",
                "prompt": "Review auth",
                "summary": "Done",
                "provider": "openai",
                "model": "gpt-5",
                "reasoningEffort": "high",
                "workflowId": "open-ended",
                "profile": {},
                "metadata": {},
                "finalDisposition": null,
                "attempts": [],
                "tokenUsage": {},
                "createdAt": "2026-08-21T22:00:00.000Z",
                "startedAt": "2026-08-21T22:00:00.000Z",
                "endedAt": "2026-08-21T23:00:00.000Z",
                "updatedAt": "2026-08-21T23:00:00.000Z",
                "revision": 1
              }]
            }
            """.data(using: .utf8)
        )
        let result = try JSONDecoder().decode(AppServerCanonicalWorkspaceSessions.self, from: data)
        XCTAssertEqual(result.workspace.workspaceId, "workspace-1")
        XCTAssertEqual(result.result.first?.title, "Review authentication boundary")
    }

    func testDecodesCanonicalTranscriptCommentary() throws {
        let data = try XCTUnwrap(
            """
            {
              "controlVersion": 1,
              "workspace": {
                "id": "registry-1",
                "workspaceId": "workspace-1",
                "name": "Example",
                "researchProfileId": "security-research",
                "researchKitId": "general",
                "runCount": 1,
                "lastRunAt": "2026-08-22T00:00:00.000Z",
                "updatedAt": "2026-08-22T00:00:00.000Z"
              },
              "result": {
                "sessionId": "session-1",
                "stream": "commentary",
                "events": [{
                  "id": "event-1",
                  "kind": "beale.transcript",
                  "timestamp": "2026-08-22T00:01:00.000Z",
                  "summary": "beale.transcript",
                  "payload": {
                    "record": {
                      "id": "transcript-1",
                      "runId": "session-1",
                      "attemptId": "attempt-1",
                      "traceEventId": "trace-1",
                      "role": "assistant",
                      "phase": "commentary",
                      "contentMarkdown": "Checking the authentication boundary.",
                      "source": "honeycrisp_commentary",
                      "metadata": {
                        "live": true,
                        "agentPath": "/root",
                        "responseId": "response-1",
                        "itemId": "thinking:0"
                      },
                      "createdAt": "2026-08-22T00:01:00.000Z"
                    }
                  }
                }],
                "eventOffset": 3,
                "nextAfterEventId": "event-1",
                "hasEarlier": true,
                "hasMore": false
              }
            }
            """.data(using: .utf8)
        )
        let result = try JSONDecoder().decode(AppServerCanonicalSessionEvents.self, from: data)
        let message = result.result.events.first?.payload.record
        XCTAssertEqual(message?.phase, "commentary")
        XCTAssertEqual(message?.contentMarkdown, "Checking the authentication boundary.")
        XCTAssertEqual(message?.metadata?.agentPath, "/root")
        XCTAssertEqual(message?.metadata?.responseId, "response-1")
        XCTAssertEqual(message?.metadata?.itemId, "thinking:0")
        XCTAssertEqual(result.result.nextAfterEventId, "event-1")
    }

    func testProjectsRootTranscriptLikeDesktop() {
        func message(
            _ id: String,
            content: String,
            source: String = "openai_reasoning_summary",
            phase: String? = nil,
            agentPath: String? = "/root",
            responseId: String? = nil,
            itemId: String? = nil,
            toolName: String? = nil,
            toolPluralTemplate: String? = nil
        ) -> AppServerTranscriptMessage {
            AppServerTranscriptMessage(
                id: id,
                runId: "session-1",
                attemptId: "attempt-1",
                traceEventId: "trace-\(id)",
                role: "assistant",
                phase: phase,
                contentMarkdown: content,
                source: source,
                metadata: .init(
                    agentPath: agentPath,
                    responseId: responseId,
                    itemId: itemId,
                    toolName: toolName,
                    toolCount: toolName == nil ? nil : 1,
                    toolPluralTemplate: toolPluralTemplate
                ),
                createdAt: "2026-08-22T00:00:\(id).000Z"
            )
        }

        let projected = AppServerTranscriptProjection.rootMessages([
            message("01", content: "Partial snapshot", responseId: "response-1", itemId: "thinking:0"),
            message("02", content: "Completed snapshot", responseId: "response-1", itemId: "thinking:0"),
            message(
                "03",
                content: "Subagent thought",
                agentPath: "/root/researcher",
                responseId: "subagent-response",
                itemId: "thinking:0"
            ),
            message("04", content: "Duplicated reasoning", responseId: "response-2", itemId: "thinking:0"),
            message(
                "05",
                content: "Native commentary",
                source: "honeycrisp_commentary",
                phase: "commentary",
                responseId: "response-2",
                itemId: "message:0"
            ),
            message("06", content: "Independent thought", responseId: "response-3", itemId: "thinking:0"),
            message(
                "07",
                content: "Commentary separator",
                source: "honeycrisp_commentary",
                phase: "commentary",
                responseId: "response-4",
                itemId: "message:0"
            ),
            message("08", content: "Interrupted thought", responseId: "response-5", itemId: "thinking:0"),
            message(
                "09",
                content: "**Issue** Research agent failed: WebSocket error",
                responseId: "honeycrisp-progress",
                itemId: "failure-event"
            ),
            message(
                "10",
                content: "Read parser.c",
                source: "honeycrisp_tool_summary",
                phase: "tool",
                toolName: "file.read",
                toolPluralTemplate: "Read {count} files"
            ),
            message(
                "11",
                content: "Read token.c",
                source: "honeycrisp_tool_summary",
                phase: "tool",
                toolName: "file.read",
                toolPluralTemplate: "Read {count} files"
            )
        ])

        XCTAssertEqual(projected.map(\.contentMarkdown), [
            "Completed snapshot",
            "Native commentary",
            "Independent thought",
            "Commentary separator",
            "Interrupted thought",
            "**Issue** Research agent failed: WebSocket error",
            "Read 2 files"
        ])
        XCTAssertEqual(projected.last?.metadata?.toolCount, 2)

        let subagent = AppServerTranscriptProjection.subagentMessages([
            message(
                "12",
                content: "Mapped the parser boundary.",
                source: "honeycrisp_commentary",
                phase: "commentary",
                agentPath: "/root/parser_reviewer",
                responseId: "subagent-response",
                itemId: "message:0"
            ),
            message(
                "13",
                content: "Root-only commentary",
                source: "honeycrisp_commentary",
                phase: "commentary",
                responseId: "root-response",
                itemId: "message:0"
            )
        ], path: "/root/parser_reviewer")
        XCTAssertEqual(subagent.map(\.contentMarkdown), ["Mapped the parser boundary."])
    }

    func testDecodesAndProjectsDesktopStyleSubagentSummaries() throws {
        let data = try XCTUnwrap(
            """
            {
              "controlVersion": 1,
              "workspace": {
                "id": "registry-1",
                "workspaceId": "workspace-1",
                "name": "Example",
                "researchProfileId": "security-research",
                "researchKitId": "general",
                "runCount": 1,
                "lastRunAt": null,
                "updatedAt": "2026-08-28T00:00:00.000Z"
              },
              "result": {
                "sessionId": "session-1",
                "revision": 4,
                "rooms": [],
                "members": [],
                "messages": [],
                "subagents": [
                  {
                    "id": "event-1",
                    "kind": "agent.event",
                    "timestamp": "2026-08-28T00:00:01.000Z",
                    "summary": "spawned",
                    "payload": {
                      "type": "subagent.activity",
                      "action": "spawned",
                      "agentId": "agent-parser",
                      "agentPath": "/root/parser_reviewer",
                      "provider": "openai-codex",
                      "model": "gpt-5.6-sol",
                      "status": "running",
                      "channelName": "parser-review"
                    }
                  },
                  {
                    "id": "event-2",
                    "kind": "agent.event",
                    "timestamp": "2026-08-28T00:00:02.000Z",
                    "summary": "message",
                    "payload": {
                      "type": "subagent.activity",
                      "action": "message",
                      "agentPath": "/root/parser_reviewer",
                      "message": "Mapped the request framing boundary."
                    }
                  },
                  {
                    "id": "event-3",
                    "kind": "agent.event",
                    "timestamp": "2026-08-28T00:00:03.000Z",
                    "summary": "completed",
                    "payload": {
                      "type": "subagent.activity",
                      "action": "completed",
                      "agentPath": "/root/parser_reviewer",
                      "status": "completed",
                      "message": "Confirmed the downstream parser consumer."
                    }
                  }
                ]
              }
            }
            """.data(using: .utf8)
        )
        let response = try JSONDecoder().decode(AppServerCanonicalSessionCollaboration.self, from: data)
        let summaries = AppServerSubagentProjection.summaries(
            from: response.result.subagents,
            sessionStatus: "active"
        )

        XCTAssertEqual(summaries.count, 1)
        XCTAssertEqual(summaries[0].path, "/root/parser_reviewer")
        XCTAssertEqual(summaries[0].status, "completed")
        XCTAssertEqual(summaries[0].latestMessage, "Confirmed the downstream parser consumer.")
        XCTAssertEqual(summaries[0].provider, "openai-codex")
        XCTAssertEqual(summaries[0].model, "gpt-5.6-sol")
        XCTAssertEqual(summaries[0].channelName, "parser-review")
    }

    func testDecodesPathFreeHeatBearingMemoryFeed() throws {
        let data = try XCTUnwrap(
            """
            {
              "controlVersion": 1,
              "workspace": {
                "id": "registry-1",
                "workspaceId": "workspace-1",
                "name": "Example",
                "researchProfileId": "security-research",
                "researchKitId": "general",
                "runCount": 1,
                "lastRunAt": "2026-08-22T00:00:00.000Z",
                "updatedAt": "2026-08-22T00:00:00.000Z"
              },
              "result": {
                "schemaVersion": 3,
                "workspaceId": "workspace-1",
                "profile": {
                  "id": "security-research",
                  "version": "1",
                  "hash": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                },
                "nodes": [{
                  "id": "memory-1",
                  "kind": "claim",
                  "sessionIds": ["session-1"],
                  "type": "security.chain",
                  "typeName": "Composite finding",
                  "title": "Authenticated attack chain",
                  "summary": "The chain reaches a privileged sink.",
                  "status": "verified",
                  "heat": "critical",
                  "rating": "high",
                  "createdAt": "2026-08-22T00:01:00.000Z",
                  "updatedAt": "2026-08-22T00:01:00.000Z",
                  "revision": 1
                }]
              }
            }
            """.data(using: .utf8)
        )
        let result = try JSONDecoder().decode(AppServerCanonicalMemoryNotifications.self, from: data)
        XCTAssertNoThrow(try result.validatedResult(workspaceId: "workspace-1"))
        XCTAssertEqual(result.result.nodes.first?.heat, "critical")
        XCTAssertEqual(result.result.nodes.first?.rating, "high")
        XCTAssertEqual(result.result.nodes.first?.kind, "claim")
        XCTAssertEqual(result.result.nodes.first?.typeName, "Composite finding")
        XCTAssertEqual(result.result.nodes.first?.notificationCheckpoint, "memory-1|1|critical|high")
        let raw = try XCTUnwrap(String(data: data, encoding: .utf8))
        XCTAssertFalse(raw.contains("databasePath"))
        XCTAssertFalse(raw.contains("workspacePath"))
        XCTAssertFalse(raw.contains("artifactDirectoryPath"))
    }

    func testIOSNotificationPolicyOnlyAllowsMediumAndHighFindingMilestones() {
        func node(
            kind: String = "claim",
            status: String,
            rating: String?
        ) -> AppServerMemoryNotificationNode {
            AppServerMemoryNotificationNode(
                id: "claim-\(status)-\(rating ?? "none")",
                kind: kind,
                sessionIds: ["session-1"],
                type: "security.primitive",
                typeName: "Finding",
                title: "Authorization boundary",
                summary: "A bounded summary.",
                status: status,
                heat: "high",
                rating: rating,
                createdAt: "2026-08-28T00:00:00.000Z",
                updatedAt: "2026-08-28T00:00:00.000Z",
                revision: 1
            )
        }

        XCTAssertTrue(node(status: "observed", rating: "medium").isEligibleForIOSNotification)
        XCTAssertTrue(node(status: "observed", rating: "high").isEligibleForIOSNotification)
        XCTAssertTrue(node(status: "reproduced", rating: "medium").isEligibleForIOSNotification)
        XCTAssertTrue(node(status: "verified", rating: "high").isEligibleForIOSNotification)

        XCTAssertFalse(node(status: "hypothesis", rating: "medium").isEligibleForIOSNotification)
        XCTAssertFalse(node(status: "report_ready", rating: "high").isEligibleForIOSNotification)
        XCTAssertFalse(node(status: "stale", rating: "high").isEligibleForIOSNotification)
        XCTAssertFalse(node(status: "observed", rating: "informational").isEligibleForIOSNotification)
        XCTAssertFalse(node(status: "observed", rating: "low").isEligibleForIOSNotification)
        XCTAssertFalse(node(status: "verified", rating: "critical").isEligibleForIOSNotification)
        XCTAssertFalse(node(kind: "memory", status: "verified", rating: "high").isEligibleForIOSNotification)
        XCTAssertFalse(node(status: "verified", rating: nil).isEligibleForIOSNotification)
    }

    func testDecodesPathFreeWorkspaceMemoryCatalog() throws {
        let data = try XCTUnwrap(
            """
            {
              "controlVersion": 1,
              "workspace": {
                "id": "registry-1",
                "workspaceId": "workspace-1",
                "name": "Example",
                "researchProfileId": "security-research",
                "researchKitId": "general",
                "runCount": 1,
                "lastRunAt": "2026-08-22T00:00:00.000Z",
                "updatedAt": "2026-08-22T00:00:00.000Z"
              },
              "result": {
                "schemaVersion": 3,
                "workspaceId": "workspace-1",
                "status": "ready",
                "nodeCount": 1,
                "nodeTypeCounts": {"finding": 1},
                "nodes": [{
                  "id": "memory-1",
                  "sessionIds": ["session-1"],
                  "type": "finding",
                  "title": "Authentication boundary",
                  "summary": "A privileged boundary needs further verification.",
                  "status": "candidate",
                  "confidence": 0.75,
                  "tags": ["auth"],
                  "createdAt": "2026-08-22T00:01:00.000Z",
                  "updatedAt": "2026-08-22T00:02:00.000Z",
                  "revision": 2
                }],
                "leads": [{
                  "id": "claim-lead",
                  "sessionIds": ["session-1"],
                  "projection": "lead",
                  "maturity": "proposed",
                  "freshness": "current",
                  "workflow": "open",
                  "classification": "security.vulnerability",
                  "componentClaimIds": [],
                  "title": "Authorization lead",
                  "summary": "A boundary may be bypassable.",
                  "impact": "Unknown",
                  "rating": "low",
                  "confidence": 0.4,
                  "evidenceCount": 0,
                  "createdAt": "2026-08-22T00:01:00.000Z",
                  "updatedAt": "2026-08-22T00:02:00.000Z",
                  "revision": 1
                }],
                "findings": [{
                  "id": "claim-finding",
                  "sessionIds": ["session-1"],
                  "projection": "finding",
                  "maturity": "observed",
                  "freshness": "current",
                  "workflow": "open",
                  "classification": "security.primitive",
                  "componentClaimIds": [],
                  "title": "Observed authorization bypass",
                  "summary": "A test reached the privileged operation.",
                  "impact": "Privileged operation is reachable.",
                  "rating": "high",
                  "confidence": 0.8,
                  "evidenceCount": 1,
                  "createdAt": "2026-08-22T00:01:00.000Z",
                  "updatedAt": "2026-08-22T00:02:00.000Z",
                  "revision": 2
                }]
              }
            }
            """.data(using: .utf8)
        )
        let result = try JSONDecoder().decode(AppServerCanonicalWorkspaceMemory.self, from: data)
        XCTAssertNoThrow(try result.validatedResult(workspaceId: "workspace-1"))
        XCTAssertEqual(result.result.nodeCount, 1)
        XCTAssertEqual(result.result.nodes.first?.title, "Authentication boundary")
        XCTAssertEqual(result.result.nodes.first?.confidence, 0.75)
        XCTAssertEqual(result.result.leads.first?.id, "claim-lead")
        XCTAssertEqual(result.result.leads.first?.rating, "low")
        XCTAssertEqual(result.result.findings.first?.classification, "security.primitive")
        XCTAssertEqual(result.result.findings.first?.rating, "high")
        let raw = try XCTUnwrap(String(data: data, encoding: .utf8))
        XCTAssertFalse(raw.contains("databasePath"))
        XCTAssertFalse(raw.contains("workspacePath"))
        XCTAssertFalse(raw.contains("artifactDirectoryPath"))
    }

    func testRejectsRetiredWorkspaceMemorySchema() throws {
        let result = AppServerCanonicalWorkspaceMemory(
            controlVersion: BealeAppServerContract.controlVersion,
            workspace: AppServerWorkspace(
                id: "registry-1",
                workspaceId: "workspace-1",
                name: "Example",
                researchProfileId: "security-research",
                researchKitId: "general",
                runCount: 1,
                lastRunAt: nil,
                updatedAt: "2026-08-22T00:00:00.000Z"
            ),
            result: AppServerWorkspaceMemoryCatalog(
                schemaVersion: 2,
                workspaceId: "workspace-1",
                status: "ready",
                nodeCount: 0,
                nodeTypeCounts: [:],
                nodes: [],
                leads: [],
                findings: []
            )
        )

        XCTAssertThrowsError(try result.validatedResult(workspaceId: "workspace-1")) { error in
            XCTAssertEqual(
                error as? AppServerClientError,
                .incompatible("The workspace memory response did not match this request.")
            )
        }
    }

    func testProjectsPendingSessionApprovalFromAppendOrderedEvents() throws {
        let data = try XCTUnwrap(
            """
            {
              "controlVersion": 1,
              "workspace": {
                "id": "registry-1",
                "workspaceId": "workspace-1",
                "name": "Example",
                "researchProfileId": "security-research",
                "researchKitId": "general",
                "runCount": 1,
                "lastRunAt": "2026-08-22T00:00:00.000Z",
                "updatedAt": "2026-08-22T00:00:00.000Z"
              },
              "result": {
                "sessionId": "session-1",
                "stream": "trace",
                "events": [
                  {
                    "id": "event-z",
                    "kind": "beale.approval",
                    "timestamp": "2026-08-22T00:01:00.000Z",
                    "payload": {"record": {
                      "id": "approval-1",
                      "requestKind": "shell_command",
                      "requestedAction": {
                        "approvalRequestId": "shell-1",
                        "approvalKind": "auto_review_override",
                        "reviewReason": "The command needs human review.",
                        "command": {"utility": "xcodebuild", "args": ["test"]}
                      },
                      "decision": "pending",
                      "createdAt": "2026-08-22T00:01:00.000Z"
                    }}
                  },
                  {
                    "id": "event-a",
                    "kind": "beale.approval",
                    "timestamp": "2026-08-22T00:01:00.000Z",
                    "payload": {"record": {
                      "id": "approval-1",
                      "requestKind": "shell_command",
                      "requestedAction": {"approvalRequestId": "shell-1"},
                      "decision": "approved",
                      "createdAt": "2026-08-22T00:01:00.000Z"
                    }}
                  },
                  {
                    "id": "event-tool",
                    "kind": "agent.event",
                    "timestamp": "2026-08-22T00:02:00.000Z",
                    "payload": {
                      "type": "tool_authorization_requested",
                      "approvalRequestId": "tool-1",
                      "toolName": "act",
                      "arguments": {"process": "Simulator"}
                    }
                  }
                ]
              }
            }
            """.data(using: .utf8)
        )
        let result = try JSONDecoder().decode(AppServerCanonicalSessionApprovalEvents.self, from: data)
        let pending = AppServerApprovalProjection.pendingApprovals(result.result.events)
        XCTAssertEqual(pending.count, 1)
        XCTAssertEqual(pending.first?.id, "tool-1")
        XCTAssertTrue(pending.first?.isComputerUse ?? false)
        XCTAssertEqual(pending.first?.targetBinary, "Simulator")
    }

    func testDeclaresBackgroundMemoryRefreshTask() throws {
        let identifiers = Bundle.main.object(forInfoDictionaryKey: "BGTaskSchedulerPermittedIdentifiers") as? [String]
        XCTAssertEqual(identifiers, [AppModel.memoryRefreshTaskIdentifier])
        let modes = Bundle.main.object(forInfoDictionaryKey: "UIBackgroundModes") as? [String]
        XCTAssertTrue(modes?.contains("fetch") ?? false)
    }
}
