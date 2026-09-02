import SwiftUI
import UIKit
import UserNotifications

private enum BealeTheme {
    static let chrome = Color(red: 21 / 255, green: 21 / 255, blue: 21 / 255)
    static let panel = Color(red: 13 / 255, green: 13 / 255, blue: 13 / 255)
    static let text = Color(red: 242 / 255, green: 242 / 255, blue: 242 / 255)
    static let muted = Color(red: 150 / 255, green: 150 / 255, blue: 150 / 255)
}

struct ContentView: View {
    @ObservedObject var model: AppModel
    @Environment(\.scenePhase) private var scenePhase
    @State private var navigationPath = NavigationPath()
    @State private var showingSettings = false

    var body: some View {
        NavigationStack(path: $navigationPath) {
            Group {
                if model.isConnected {
                    WorkspacesView(model: model)
                } else {
                    ConnectionChooserView(model: model)
                }
            }
            .navigationTitle(model.isConnected ? "Workspaces" : "Connect")
            .navigationBarTitleDisplayMode(.inline)
            .toolbarBackground(BealeTheme.chrome, for: .navigationBar)
            .toolbarBackground(.visible, for: .navigationBar)
            .toolbar {
                if model.isConnected {
                    ToolbarItem(placement: .topBarLeading) {
                        Menu {
                            Button {
                                navigationPath = NavigationPath()
                            } label: {
                                Label("Workspaces", systemImage: "folder")
                            }
                            Button {
                                showingSettings = true
                            } label: {
                                Label("Settings", systemImage: "gearshape")
                            }
                        } label: {
                            Image(systemName: "line.3.horizontal")
                        }
                        .accessibilityLabel("Menu")
                    }

                }
            }
        }
        .sheet(isPresented: $showingSettings) {
            NavigationStack {
                BealeSettingsView(model: model)
                    .toolbar {
                        ToolbarItem(placement: .confirmationAction) {
                            Button("Done") { showingSettings = false }
                        }
                    }
            }
        }
        .task {
            await model.refreshMemoryNotificationAuthorization()
        }
        .task(id: model.memoryNotificationsEnabled && model.isConnected) {
            await model.followMemoryNotifications()
        }
        .onChange(of: scenePhase) { _, phase in
            if phase == .background {
                model.scheduleBackgroundMemoryRefresh()
            } else if phase == .active {
                Task { await model.refreshMemoryNotificationAuthorization() }
            }
        }
        .onChange(of: model.isConnected) { _, _ in
            navigationPath = NavigationPath()
        }
        .tint(BealeTheme.text)
        .preferredColorScheme(.dark)
    }
}

private struct WorkspacesView: View {
    @ObservedObject var model: AppModel
    @State private var expandedWorkspaceIds: Set<String> = []

    var body: some View {
        List {
            if model.workspaces.isEmpty {
                ContentUnavailableView(
                    "No Workspaces",
                    systemImage: "folder.badge.questionmark",
                    description: Text("No workspaces are registered on this Beale host.")
                )
                .listRowBackground(BealeTheme.panel)
            } else {
                ForEach(model.workspaces) { workspace in
                    workspaceSection(workspace)
                        .task {
                            await model.loadSessions(for: workspace)
                            await model.loadMemory(for: workspace)
                        }
                }
            }
        }
        .navigationTitle("Workspaces")
        .navigationBarTitleDisplayMode(.inline)
        .toolbarBackground(BealeTheme.chrome, for: .navigationBar)
        .toolbarBackground(.visible, for: .navigationBar)
        .scrollContentBackground(.hidden)
        .background(BealeTheme.panel)
        .refreshable {
            await model.refreshWorkspaces()
        }
    }

    @ViewBuilder
    private func workspaceSection(_ workspace: AppServerWorkspace) -> some View {
        let sessions = model.sessions(for: workspace)
        let isExpanded = expandedWorkspaceIds.contains(workspace.workspaceId)
        let visibleSessions = isExpanded ? sessions : Array(sessions.prefix(3))

        Section {
            NavigationLink {
                RemoteWorkspaceView(workspace: workspace, model: model)
            } label: {
                Label {
                    VStack(alignment: .leading, spacing: 3) {
                        Text(workspace.name)
                            .foregroundStyle(BealeTheme.text)
                        Text("\(workspace.runCount) \(workspace.runCount == 1 ? "run" : "runs")")
                            .font(.caption)
                            .foregroundStyle(BealeTheme.muted)
                    }
                } icon: {
                    Image(systemName: "folder")
                }
            }

            if model.isLoadingSessions(for: workspace), sessions.isEmpty {
                HStack(spacing: 10) {
                    ProgressView()
                    Text("Loading sessions")
                        .foregroundStyle(BealeTheme.muted)
                }
                .font(.subheadline)
            } else if let error = model.sessionError(for: workspace), sessions.isEmpty {
                Label(error, systemImage: "exclamationmark.triangle")
                    .font(.caption)
                    .foregroundStyle(.orange)
            } else if sessions.isEmpty {
                Text("No sessions")
                    .font(.subheadline)
                    .foregroundStyle(BealeTheme.muted)
            } else {
                ForEach(visibleSessions) { session in
                    NavigationLink {
                        RemoteSessionView(workspace: workspace, session: session, model: model)
                    } label: {
                        RemoteSessionRow(session: session)
                    }
                }

                if sessions.count > 3 {
                    Button {
                        withAnimation {
                            if isExpanded {
                                expandedWorkspaceIds.remove(workspace.workspaceId)
                            } else {
                                expandedWorkspaceIds.insert(workspace.workspaceId)
                            }
                        }
                    } label: {
                        Label(
                            isExpanded ? "Show Three Most Recent" : "Show All \(sessions.count) Sessions",
                            systemImage: isExpanded ? "chevron.up" : "chevron.down"
                        )
                        .font(.subheadline)
                    }
                }
            }
        }
    }
}

private struct RemoteWorkspaceView: View {
    let workspace: AppServerWorkspace
    @ObservedObject var model: AppModel
    @State private var showingNewSession = false
    @State private var pendingStartedSession: AppServerWorkspaceSession?
    @State private var openedSession: AppServerWorkspaceSession?

    var body: some View {
        List {
            Section("Workspace") {
                LabeledContent("Research Profile", value: workspace.researchProfileId)
                LabeledContent("Research Kit", value: workspace.researchKitId)
                LabeledContent("Runs", value: String(workspace.runCount))
                NavigationLink {
                    WorkspaceClaimsView(workspace: workspace, model: model)
                } label: {
                    if let catalog = model.memoryCatalog(for: workspace) {
                        Label(
                            "\(catalog.leads.count + catalog.findings.count) Claims · \(catalog.leads.count) Leads · \(catalog.findings.count) Findings",
                            systemImage: "doc.text.magnifyingglass"
                        )
                    } else {
                        HStack {
                            Label("Claims", systemImage: "doc.text.magnifyingglass")
                            Spacer()
                            if model.isLoadingMemory(for: workspace) {
                                ProgressView()
                                    .controlSize(.small)
                            }
                        }
                    }
                }
                NavigationLink {
                    WorkspaceMemoriesView(workspace: workspace, model: model)
                } label: {
                    if let catalog = model.memoryCatalog(for: workspace) {
                        Label(
                            "\(catalog.nodeCount) \(catalog.nodeCount == 1 ? "Memory" : "Memories")",
                            systemImage: "internaldrive"
                        )
                    } else {
                        HStack {
                            Label("Memories", systemImage: "internaldrive")
                            Spacer()
                            if model.isLoadingMemory(for: workspace) {
                                ProgressView()
                                    .controlSize(.small)
                            }
                        }
                    }
                }
            }

            Section("Sessions") {
                if model.isLoadingSessions(for: workspace), model.sessions(for: workspace).isEmpty {
                    HStack(spacing: 10) {
                        ProgressView()
                        Text("Loading sessions")
                            .foregroundStyle(BealeTheme.muted)
                    }
                } else if let error = model.sessionError(for: workspace), model.sessions(for: workspace).isEmpty {
                    Label(error, systemImage: "exclamationmark.triangle")
                        .font(.caption)
                        .foregroundStyle(.orange)
                } else if model.sessions(for: workspace).isEmpty {
                    Text("No sessions")
                        .foregroundStyle(BealeTheme.muted)
                } else {
                    ForEach(model.sessions(for: workspace)) { session in
                        NavigationLink {
                            RemoteSessionView(workspace: workspace, session: session, model: model)
                        } label: {
                            RemoteSessionRow(session: session)
                        }
                    }
                }
            }
        }
        .navigationTitle(workspace.name)
        .navigationBarTitleDisplayMode(.inline)
        .toolbarBackground(BealeTheme.chrome, for: .navigationBar)
        .toolbarBackground(.visible, for: .navigationBar)
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                Button {
                    showingNewSession = true
                } label: {
                    Image(systemName: "plus")
                }
                .accessibilityLabel("New research session")
            }
        }
        .scrollContentBackground(.hidden)
        .background(BealeTheme.panel)
        .sheet(isPresented: $showingNewSession, onDismiss: openPendingSession) {
            NewWorkspaceSessionView(workspace: workspace, model: model) { session in
                pendingStartedSession = session
                showingNewSession = false
            }
        }
        .navigationDestination(
            isPresented: Binding(
                get: { openedSession != nil },
                set: { if !$0 { openedSession = nil } }
            )
        ) {
            if let openedSession {
                RemoteSessionView(workspace: workspace, session: openedSession, model: model)
            }
        }
        .task {
            await model.loadSessions(for: workspace)
            await model.loadMemory(for: workspace)
        }
        .refreshable {
            await model.loadSessions(for: workspace, force: true)
            await model.loadMemory(for: workspace, force: true)
        }
    }

    private func openPendingSession() {
        guard let pendingStartedSession else { return }
        self.pendingStartedSession = nil
        openedSession = pendingStartedSession
    }
}

private struct NewWorkspaceSessionView: View {
    let workspace: AppServerWorkspace
    @ObservedObject var model: AppModel
    let started: (AppServerWorkspaceSession) -> Void
    @Environment(\.dismiss) private var dismiss
    @State private var prompt = ""
    @State private var configuration = AppServerSessionLaunchConfiguration.defaultValue
    @State private var isStarting = false
    @State private var isShowingSuggestions = false
    @State private var isLoadingSuggestions = false
    @State private var isAddingContext = false
    @State private var addContextEnabled = false
    @State private var suggestions: [AppServerResearchPromptSuggestion] = []
    @State private var researchPhase: String?
    @State private var suggestionError: String?
    @State private var error: String?

    private var canStart: Bool {
        !prompt.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            && configurationError == nil
            && !isStarting
            && !isAddingContext
    }

    private var connectedProviders: [AppServerProviderCatalogEntry] {
        (model.providerCatalog?.providers ?? []).filter {
            AppServerResearchProvider(rawValue: $0.providerId) != nil
        }
    }

    private var leadProviderCatalog: AppServerProviderCatalogEntry? {
        let providerId = configuration.leadProvider?.rawValue ?? model.providerCatalog?.defaultProviderId
        return connectedProviders.first { $0.providerId == providerId }
    }

    private var configurationError: String? {
        if connectedProviders.isEmpty {
            return "Connect a model provider in Beale Desktop before starting research."
        }
        if configuration.collaborators.contains(where: {
            $0.model.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        }) {
            return "Enter a model ID for every collaborator."
        }
        let collaboratorKeys = configuration.collaborators.map {
            "\($0.provider.rawValue)\u{0}\($0.model.trimmingCharacters(in: .whitespacesAndNewlines))"
        }
        if Set(collaboratorKeys).count != collaboratorKeys.count {
            return "Each collaborator must use a unique provider and model combination."
        }
        return nil
    }

    private var safetyDescription: String {
        switch configuration.shellSafetyMode {
        case .manualApproval:
            "Shell commands pause for your approval."
        case .autoReview:
            "The configured review model evaluates shell commands automatically."
        case .danger:
            "Shell commands run without approval or automatic review."
        }
    }

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    TextField("What should Beale research?", text: $prompt, axis: .vertical)
                        .lineLimit(5...12)
                        .textInputAutocapitalization(.sentences)
                        .disabled(isStarting || isAddingContext)

                    Button {
                        toggleSuggestions()
                    } label: {
                        Label("Suggestions", systemImage: "lightbulb")
                            .frame(maxWidth: .infinity, alignment: .leading)
                    }
                    .disabled(isStarting || isAddingContext)

                    HStack(spacing: 16) {
                        Button {
                            addContextEnabled.toggle()
                        } label: {
                            Label(
                                "Add Context",
                                systemImage: addContextEnabled ? "checkmark.square.fill" : "square"
                            )
                        }
                        .accessibilityAddTraits(addContextEnabled ? .isSelected : [])
                        .disabled(isStarting || isAddingContext)

                        Button {
                            configuration.goalEnabled.toggle()
                        } label: {
                            Label(
                                "Goal",
                                systemImage: configuration.goalEnabled ? "checkmark.square.fill" : "square"
                            )
                        }
                        .accessibilityAddTraits(configuration.goalEnabled ? .isSelected : [])
                        .disabled(isStarting || isAddingContext)
                    }

                    if isShowingSuggestions {
                        if isLoadingSuggestions {
                            HStack {
                                Spacer()
                                ProgressView("Finding suggestions…")
                                Spacer()
                            }
                        } else if let suggestionError {
                            Label(suggestionError, systemImage: "exclamationmark.triangle")
                                .font(.caption)
                                .foregroundStyle(.orange)
                        } else {
                            ForEach(suggestions) { suggestion in
                                Button {
                                    prompt = suggestion.promptMarkdown
                                    configuration.goalObjective = nil
                                    isShowingSuggestions = false
                                } label: {
                                    VStack(alignment: .leading, spacing: 5) {
                                        Text(suggestion.title)
                                            .multilineTextAlignment(.leading)
                                        if let rationale = suggestion.rationale {
                                            Text(rationale)
                                                .font(.caption)
                                                .foregroundStyle(.secondary)
                                                .multilineTextAlignment(.leading)
                                        }
                                    }
                                    .frame(maxWidth: .infinity, alignment: .leading)
                                }
                            }
                        }
                    }
                } header: {
                    Text("Research Request")
                } footer: {
                    Text(isAddingContext
                        ? "Adding useful context…"
                        : "Suggestions fill the request. Add Context grounds it in this workspace before launch. Goal keeps the session working across turns until complete or genuinely blocked.")
                }

                Section {
                    Picker("Provider", selection: $configuration.leadProvider) {
                        Text(defaultProviderLabel).tag(AppServerResearchProvider?.none)
                        ForEach(connectedProviders) { catalog in
                            if let provider = AppServerResearchProvider(rawValue: catalog.providerId) {
                                Text(catalog.providerName).tag(Optional(provider))
                            }
                        }
                    }
                    .onChange(of: configuration.leadProvider) { _, _ in
                        configuration.leadModel = ""
                    }
                    if let catalog = leadProviderCatalog {
                        Picker("Model", selection: $configuration.leadModel) {
                            Text(defaultModelLabel(for: catalog, kind: .lead)).tag("")
                            ForEach(catalog.models) { providerModel in
                                Text(modelLabel(
                                    providerModel,
                                    defaultModelId: catalog.defaultLeadModel
                                )).tag(providerModel.id)
                            }
                        }
                    }
                } header: {
                    Text("Lead Model")
                } footer: {
                    Text("Defaults come from Provider Settings on the connected Beale host.")
                }

                Section {
                    Picker("Subagent Mode", selection: $configuration.subagentMode) {
                        ForEach(AppServerSubagentMode.allCases) { mode in
                            Text(mode.displayName).tag(mode)
                        }
                    }
                    .disabled(configuration.collaborators.isEmpty || isStarting)

                    Button {
                        addCollaborator()
                    } label: {
                        Label("Add Collaborator", systemImage: "plus")
                    }
                    .disabled(
                        connectedProviders.isEmpty
                            || configuration.collaborators.count >= 5
                            || isStarting
                    )
                } header: {
                    Text("Collaborators")
                } footer: {
                    Text(configuration.collaborators.isEmpty
                        ? "No collaborator subagents will be configured."
                        : "Simple shares all roles; Advanced assigns compatible specialist roles.")
                }

                ForEach($configuration.collaborators) { $collaborator in
                    Section {
                        Picker("Provider", selection: $collaborator.provider) {
                            ForEach(connectedProviders) { catalog in
                                if let provider = AppServerResearchProvider(rawValue: catalog.providerId) {
                                    Text(catalog.providerName).tag(provider)
                                }
                            }
                        }
                        .onChange(of: collaborator.provider) { _, provider in
                            resetCollaborator(id: collaborator.id, provider: provider)
                        }
                        if let catalog = providerCatalog(for: collaborator.provider) {
                            Picker("Model", selection: $collaborator.model) {
                                ForEach(catalog.models) { providerModel in
                                    Text(modelLabel(
                                        providerModel,
                                        defaultModelId: catalog.defaultSubagentModel
                                    )).tag(providerModel.id)
                                }
                            }
                            .onChange(of: collaborator.model) { _, _ in
                                resetCollaboratorEffort(id: collaborator.id)
                            }
                        }
                        Picker("Reasoning", selection: $collaborator.reasoningEffort) {
                            ForEach(availableEfforts(for: collaborator)) { effort in
                                Text(effort.displayName).tag(effort)
                            }
                        }
                        Button("Remove Collaborator", role: .destructive) {
                            configuration.collaborators.removeAll { $0.id == collaborator.id }
                        }
                    } header: {
                        Text("Subagent Model")
                    }
                    .disabled(isStarting)
                }

                Section {
                    Picker("Safety Mode", selection: $configuration.shellSafetyMode) {
                        ForEach(AppServerShellSafetyMode.allCases) { mode in
                            Text(mode.displayName).tag(mode)
                        }
                    }
                    .disabled(isStarting)
                } header: {
                    Text("Shell Safety")
                } footer: {
                    Text(safetyDescription)
                }

                if let configurationError {
                    Section {
                        Label(configurationError, systemImage: "exclamationmark.triangle")
                            .font(.caption)
                            .foregroundStyle(.orange)
                    }
                }

                if let error {
                    Section {
                        Label(error, systemImage: "exclamationmark.triangle")
                            .font(.caption)
                            .foregroundStyle(.orange)
                    }
                }
            }
            .navigationTitle("New Research")
            .navigationBarTitleDisplayMode(.inline)
            .toolbarBackground(BealeTheme.chrome, for: .navigationBar)
            .toolbarBackground(.visible, for: .navigationBar)
            .scrollContentBackground(.hidden)
            .background(BealeTheme.panel)
            .interactiveDismissDisabled(isStarting || isAddingContext)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { dismiss() }
                        .disabled(isStarting || isAddingContext)
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button {
                        startSession()
                    } label: {
                        if isStarting {
                            ProgressView()
                                .controlSize(.small)
                        } else {
                            Text(addContextEnabled ? "Add Context & Start" : "Start")
                        }
                    }
                    .disabled(!canStart)
                }
            }
        }
        .preferredColorScheme(.dark)
    }

    private enum DefaultModelKind {
        case lead
        case subagent
    }

    private var defaultProviderLabel: String {
        guard let defaultProviderId = model.providerCatalog?.defaultProviderId,
              let provider = connectedProviders.first(where: { $0.providerId == defaultProviderId }) else {
            return "Host Default"
        }
        return "Host Default · \(provider.providerName)"
    }

    private func defaultModelLabel(
        for catalog: AppServerProviderCatalogEntry,
        kind: DefaultModelKind
    ) -> String {
        let modelId = kind == .lead ? catalog.defaultLeadModel : catalog.defaultSubagentModel
        guard let modelId else { return "Host Default" }
        let name = catalog.models.first(where: { $0.id == modelId })?.name ?? modelId
        return "Host Default · \(name)"
    }

    private func modelLabel(_ model: AppServerProviderModel, defaultModelId: String?) -> String {
        let identity = model.name == model.id ? model.name : "\(model.name) · \(model.id)"
        return model.id == defaultModelId ? "\(identity) (Default)" : identity
    }

    private func providerCatalog(
        for provider: AppServerResearchProvider
    ) -> AppServerProviderCatalogEntry? {
        connectedProviders.first { $0.providerId == provider.rawValue }
    }

    private func addCollaborator() {
        guard let catalog = leadProviderCatalog ?? connectedProviders.first,
              let provider = AppServerResearchProvider(rawValue: catalog.providerId),
              let modelId = catalog.defaultSubagentModel
                ?? catalog.defaultLeadModel
                ?? catalog.models.first?.id else {
            return
        }
        let collaborator = AppServerSessionCollaborator(
            provider: provider,
            model: modelId,
            reasoningEffort: preferredEffort(
                catalog.defaultReasoningEffort,
                supported: catalog.models.first(where: { $0.id == modelId })?.effortLevels ?? []
            )
        )
        configuration.collaborators.append(collaborator)
    }

    private func resetCollaborator(id: UUID, provider: AppServerResearchProvider) {
        guard let index = configuration.collaborators.firstIndex(where: { $0.id == id }),
              let catalog = providerCatalog(for: provider),
              let modelId = catalog.defaultSubagentModel
                ?? catalog.defaultLeadModel
                ?? catalog.models.first?.id else {
            return
        }
        configuration.collaborators[index].model = modelId
        configuration.collaborators[index].reasoningEffort = preferredEffort(
            catalog.defaultReasoningEffort,
            supported: catalog.models.first(where: { $0.id == modelId })?.effortLevels ?? []
        )
    }

    private func resetCollaboratorEffort(id: UUID) {
        guard let index = configuration.collaborators.firstIndex(where: { $0.id == id }),
              let catalog = providerCatalog(for: configuration.collaborators[index].provider),
              let selectedModel = catalog.models.first(where: {
                  $0.id == configuration.collaborators[index].model
              }) else {
            return
        }
        configuration.collaborators[index].reasoningEffort = preferredEffort(
            catalog.defaultReasoningEffort,
            supported: selectedModel.effortLevels
        )
    }

    private func availableEfforts(
        for collaborator: AppServerSessionCollaborator
    ) -> [AppServerReasoningEffort] {
        guard let model = providerCatalog(for: collaborator.provider)?.models.first(where: {
            $0.id == collaborator.model
        }) else {
            return AppServerReasoningEffort.allCases
        }
        let supported = model.effortLevels.compactMap(AppServerReasoningEffort.init(rawValue:))
        return supported.isEmpty ? AppServerReasoningEffort.allCases : supported
    }

    private func preferredEffort(
        _ hostDefault: String?,
        supported effortLevels: [String]
    ) -> AppServerReasoningEffort {
        let supported = Set(effortLevels.compactMap(AppServerReasoningEffort.init(rawValue:)))
        if let hostDefault,
           let effort = AppServerReasoningEffort(rawValue: hostDefault),
           supported.contains(effort) {
            return effort
        }
        return [.high, .xhigh, .medium, .low, .minimal, .max]
            .first(where: { supported.contains($0) }) ?? .high
    }

    private func startSession() {
        guard canStart else { return }
        isStarting = true
        error = nil
        Task {
            do {
                var launchPrompt = prompt.trimmingCharacters(in: .whitespacesAndNewlines)
                if addContextEnabled {
                    isAddingContext = true
                    let originalPrompt = launchPrompt
                    let expanded = try await model.addContext(
                        to: originalPrompt,
                        in: workspace,
                        phase: researchPhase,
                        configuration: configuration
                    )
                    if configuration.goalObjective == nil {
                        configuration.goalObjective = originalPrompt
                    }
                    launchPrompt = expanded.promptMarkdown
                    prompt = launchPrompt
                    isAddingContext = false
                }
                started(try await model.startSession(
                    in: workspace,
                    prompt: launchPrompt,
                    configuration: configuration
                ))
            } catch is CancellationError {
                dismiss()
            } catch {
                self.error = error.localizedDescription
                isAddingContext = false
                isStarting = false
            }
        }
    }

    private func toggleSuggestions() {
        isShowingSuggestions.toggle()
        guard isShowingSuggestions, suggestions.isEmpty, !isLoadingSuggestions else { return }
        isLoadingSuggestions = true
        suggestionError = nil
        Task {
            do {
                let generated = try await model.researchSuggestions(in: workspace)
                researchPhase = generated.phase
                suggestions = generated.fillableSuggestions
                if suggestions.isEmpty {
                    suggestionError = "No suggestions are available for this workspace yet."
                }
            } catch is CancellationError {
                isShowingSuggestions = false
            } catch {
                suggestionError = error.localizedDescription
            }
            isLoadingSuggestions = false
        }
    }

}

private struct WorkspaceMemoriesView: View {
    let workspace: AppServerWorkspace
    @ObservedObject var model: AppModel
    @State private var searchText = ""
    @State private var selectedType = "all"

    private var catalog: AppServerWorkspaceMemoryCatalog? {
        model.memoryCatalog(for: workspace)
    }

    private var types: [String] {
        Array(Set(catalog?.nodes.map(\.type) ?? [])).sorted {
            $0.localizedCaseInsensitiveCompare($1) == .orderedAscending
        }
    }

    private var filteredNodes: [AppServerWorkspaceMemoryNode] {
        let query = searchText.trimmingCharacters(in: .whitespacesAndNewlines)
        return (catalog?.nodes ?? []).filter { node in
            let matchesType = selectedType == "all" || node.type == selectedType
            let matchesText = query.isEmpty || [node.title, node.summary, node.type, node.status, node.tags.joined(separator: " ")]
                .joined(separator: "\n")
                .localizedCaseInsensitiveContains(query)
            return matchesType && matchesText
        }
    }

    var body: some View {
        List {
            Section {
                Picker("Type", selection: $selectedType) {
                    Text("All Types").tag("all")
                    ForEach(types, id: \.self) { type in
                        Text(type.memoryTypeLabel).tag(type)
                    }
                }
                .pickerStyle(.menu)
            }

            if model.isLoadingMemory(for: workspace), catalog == nil {
                HStack(spacing: 10) {
                    ProgressView()
                    Text("Loading memories")
                        .foregroundStyle(BealeTheme.muted)
                }
            } else if let error = model.memoryError(for: workspace), catalog == nil {
                Label(error, systemImage: "exclamationmark.triangle")
                    .font(.caption)
                    .foregroundStyle(.orange)
            } else if filteredNodes.isEmpty {
                ContentUnavailableView.search(text: searchText)
                    .listRowBackground(BealeTheme.panel)
            } else {
                Section("\(filteredNodes.count) \(filteredNodes.count == 1 ? "Memory" : "Memories")") {
                    ForEach(filteredNodes) { node in
                        NavigationLink {
                            WorkspaceMemoryDetailView(node: node)
                        } label: {
                            WorkspaceMemoryRow(node: node)
                        }
                    }
                }
            }
        }
        .navigationTitle("Memories")
        .navigationBarTitleDisplayMode(.inline)
        .toolbarBackground(BealeTheme.chrome, for: .navigationBar)
        .toolbarBackground(.visible, for: .navigationBar)
        .scrollContentBackground(.hidden)
        .background(BealeTheme.panel)
        .searchable(text: $searchText, prompt: "Filter memories")
        .task {
            await model.loadMemory(for: workspace)
        }
        .refreshable {
            await model.loadMemory(for: workspace, force: true)
        }
        .onChange(of: types) { _, availableTypes in
            if selectedType != "all", !availableTypes.contains(selectedType) {
                selectedType = "all"
            }
        }
    }
}

private struct WorkspaceClaimsView: View {
    let workspace: AppServerWorkspace
    @ObservedObject var model: AppModel
    @State private var searchText = ""
    @State private var selectedClassification = "all"

    private var catalog: AppServerWorkspaceMemoryCatalog? {
        model.memoryCatalog(for: workspace)
    }

    private var classifications: [String] {
        let values = (catalog?.leads.map(\.classification) ?? [])
            + (catalog?.findings.map(\.classification) ?? [])
        return Array(Set(values)).sorted {
            $0.localizedCaseInsensitiveCompare($1) == .orderedAscending
        }
    }

    private var filteredClaims: [AppServerWorkspaceResearchClaim] {
        let query = searchText.trimmingCharacters(in: .whitespacesAndNewlines)
        return ((catalog?.findings ?? []) + (catalog?.leads ?? [])).filter { claim in
            let matchesClassification = selectedClassification == "all" || claim.classification == selectedClassification
            let matchesText = query.isEmpty || [claim.title, claim.summary, claim.impact, claim.classification, claim.maturity, claim.workflow]
                .joined(separator: "\n")
                .localizedCaseInsensitiveContains(query)
            return matchesClassification && matchesText
        }
    }

    var body: some View {
        List {
            Section {
                Picker("Classification", selection: $selectedClassification) {
                    Text("All Classifications").tag("all")
                    ForEach(classifications, id: \.self) { classification in
                        Text(classification.claimClassificationLabel).tag(classification)
                    }
                }
                .pickerStyle(.menu)
            }

            if model.isLoadingMemory(for: workspace), catalog == nil {
                HStack(spacing: 10) {
                    ProgressView()
                    Text("Loading claims")
                        .foregroundStyle(BealeTheme.muted)
                }
            } else if let error = model.memoryError(for: workspace), catalog == nil {
                Label(error, systemImage: "exclamationmark.triangle")
                    .font(.caption)
                    .foregroundStyle(.orange)
            } else if filteredClaims.isEmpty {
                if searchText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty, selectedClassification == "all" {
                    ContentUnavailableView(
                        "No Claims",
                        systemImage: "doc.text.magnifyingglass",
                        description: Text("No leads or findings have been recorded in this workspace.")
                    )
                    .listRowBackground(BealeTheme.panel)
                } else {
                    ContentUnavailableView.search(text: searchText)
                        .listRowBackground(BealeTheme.panel)
                }
            } else {
                let findings = filteredClaims.filter { $0.projection == "finding" }
                let leads = filteredClaims.filter { $0.projection == "lead" }
                if !findings.isEmpty {
                    Section("\(findings.count) \(findings.count == 1 ? "Finding" : "Findings")") {
                        ForEach(findings) { claim in
                            NavigationLink {
                                WorkspaceClaimDetailView(workspace: workspace, initialClaim: claim, model: model)
                            } label: { WorkspaceClaimRow(claim: claim) }
                        }
                    }
                }
                if !leads.isEmpty {
                    Section("\(leads.count) \(leads.count == 1 ? "Lead" : "Leads")") {
                        ForEach(leads) { claim in
                            NavigationLink {
                                WorkspaceClaimDetailView(workspace: workspace, initialClaim: claim, model: model)
                            } label: { WorkspaceClaimRow(claim: claim) }
                        }
                    }
                }
            }
        }
        .navigationTitle("Claims")
        .navigationBarTitleDisplayMode(.inline)
        .toolbarBackground(BealeTheme.chrome, for: .navigationBar)
        .toolbarBackground(.visible, for: .navigationBar)
        .scrollContentBackground(.hidden)
        .background(BealeTheme.panel)
        .searchable(text: $searchText, prompt: "Filter leads or findings")
        .task {
            await model.loadMemory(for: workspace)
        }
        .refreshable {
            await model.loadMemory(for: workspace, force: true)
        }
        .onChange(of: classifications) { _, availableClassifications in
            if selectedClassification != "all", !availableClassifications.contains(selectedClassification) {
                selectedClassification = "all"
            }
        }
    }
}

private struct WorkspaceClaimRow: View {
    let claim: AppServerWorkspaceResearchClaim

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(claim.title).foregroundStyle(BealeTheme.text).lineLimit(2)
            HStack(spacing: 8) {
                Text(claim.classification.claimClassificationLabel)
                Text(claim.maturity.capitalized)
                Text(claim.rating.capitalized)
                if claim.freshness == "stale" { Text("Stale").foregroundStyle(.orange) }
            }
            .font(.caption)
            .foregroundStyle(BealeTheme.muted)
            if !claim.summary.isEmpty {
                Text(claim.summary).font(.subheadline).foregroundStyle(BealeTheme.muted).lineLimit(3)
            }
        }
        .padding(.vertical, 3)
        .accessibilityElement(children: .combine)
    }
}

private struct WorkspaceClaimDetailView: View {
    let workspace: AppServerWorkspace
    let initialClaim: AppServerWorkspaceResearchClaim
    @ObservedObject var model: AppModel
    @Environment(\.dismiss) private var dismiss
    @State private var selectedParentId = ""
    @State private var mutationBusy = false
    @State private var mutationError: String?

    private var claim: AppServerWorkspaceResearchClaim {
        let catalog = model.memoryCatalog(for: workspace)
        return ((catalog?.findings ?? []) + (catalog?.leads ?? []))
            .first { $0.id == initialClaim.id } ?? initialClaim
    }

    private var parentCandidates: [AppServerWorkspaceResearchClaim] {
        let catalog = model.memoryCatalog(for: workspace)
        return ((catalog?.findings ?? []) + (catalog?.leads ?? []))
            .filter { $0.id != claim.id }
    }

    var body: some View {
        List {
            Section(claim.projection.capitalized) {
                Text(claim.title).font(.headline)
                if !claim.summary.isEmpty { Text(markdown: claim.summary).textSelection(.enabled) }
                if !claim.impact.isEmpty { LabeledContent("Impact", value: claim.impact) }
            }
            Section("State") {
                LabeledContent("Classification", value: claim.classification.claimClassificationLabel)
                LabeledContent("Untrusted rating", value: claim.rating.capitalized)
                LabeledContent("Maturity", value: claim.maturity.capitalized)
                LabeledContent("Freshness", value: claim.freshness.capitalized)
                LabeledContent("Workflow", value: claim.workflow.capitalized)
                LabeledContent("Evidence", value: String(claim.evidenceCount))
                LabeledContent("Confidence", value: claim.confidence.formatted(.percent.precision(.fractionLength(0))))
                LabeledContent("Revision", value: String(claim.revision))
                if !claim.componentClaimIds.isEmpty {
                    LabeledContent("Components", value: String(claim.componentClaimIds.count))
                }
            }
            if !claim.duplicateClaims.isEmpty || !parentCandidates.isEmpty {
                Section("Duplicates") {
                    ForEach(claim.duplicateClaims) { duplicate in
                        HStack(spacing: 12) {
                            VStack(alignment: .leading, spacing: 3) {
                                Text(duplicate.title)
                                    .foregroundStyle(BealeTheme.text)
                                Text("\(duplicate.projection.capitalized) · \(duplicate.maturity.capitalized)")
                                    .font(.caption)
                                    .foregroundStyle(BealeTheme.muted)
                            }
                            Spacer(minLength: 8)
                            Button("Undo") { undo(duplicate) }
                                .disabled(mutationBusy)
                        }
                    }
                    if !parentCandidates.isEmpty {
                        Picker("Mark this claim as a duplicate of", selection: $selectedParentId) {
                            Text("Choose a canonical claim").tag("")
                            ForEach(parentCandidates) { candidate in
                                Text(candidate.title).tag(candidate.id)
                            }
                        }
                        Button("Mark Duplicate") { markDuplicate() }
                            .disabled(mutationBusy || selectedParentId.isEmpty)
                    }
                    if let mutationError {
                        Text(mutationError)
                            .font(.caption)
                            .foregroundStyle(.orange)
                    }
                }
            }
        }
        .navigationTitle(claim.projection.capitalized)
        .navigationBarTitleDisplayMode(.inline)
        .toolbarBackground(BealeTheme.chrome, for: .navigationBar)
        .toolbarBackground(.visible, for: .navigationBar)
        .scrollContentBackground(.hidden)
        .background(BealeTheme.panel)
    }

    private func markDuplicate() {
        guard !selectedParentId.isEmpty else { return }
        mutationBusy = true
        mutationError = nil
        Task {
            do {
                try await model.markClaimDuplicate(
                    in: workspace,
                    claimId: claim.id,
                    parentClaimId: selectedParentId,
                    expectedRevision: claim.revision
                )
                dismiss()
            } catch {
                mutationError = error.localizedDescription
            }
            mutationBusy = false
        }
    }

    private func undo(_ duplicate: AppServerWorkspaceResearchClaimDuplicate) {
        mutationBusy = true
        mutationError = nil
        Task {
            do {
                try await model.undoClaimDuplicate(
                    in: workspace,
                    claimId: duplicate.id,
                    expectedRevision: duplicate.revision
                )
            } catch {
                mutationError = error.localizedDescription
            }
            mutationBusy = false
        }
    }
}

private struct WorkspaceMemoryRow: View {
    let node: AppServerWorkspaceMemoryNode

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(node.title)
                .foregroundStyle(BealeTheme.text)
                .lineLimit(2)
            HStack(spacing: 8) {
                Text(node.type.memoryTypeLabel)
                Text(node.status.capitalized)
            }
            .font(.caption)
            .foregroundStyle(BealeTheme.muted)
            if !node.summary.isEmpty {
                Text(node.summary)
                    .font(.subheadline)
                    .foregroundStyle(BealeTheme.muted)
                    .lineLimit(3)
            }
        }
        .padding(.vertical, 3)
        .accessibilityElement(children: .combine)
    }
}

private struct WorkspaceMemoryDetailView: View {
    let node: AppServerWorkspaceMemoryNode

    var body: some View {
        List {
            Section("Memory") {
                Text(node.title)
                    .font(.headline)
                if !node.summary.isEmpty {
                    Text(markdown: node.summary)
                        .textSelection(.enabled)
                }
            }
            Section("Details") {
                LabeledContent("Type", value: node.type.memoryTypeLabel)
                LabeledContent("Status", value: node.status.capitalized)
                LabeledContent("Confidence", value: node.confidence.formatted(.percent.precision(.fractionLength(0))))
                LabeledContent("Revision", value: String(node.revision))
                if !node.tags.isEmpty {
                    LabeledContent("Tags", value: node.tags.joined(separator: ", "))
                }
            }
        }
        .navigationTitle("Memory")
        .navigationBarTitleDisplayMode(.inline)
        .toolbarBackground(BealeTheme.chrome, for: .navigationBar)
        .toolbarBackground(.visible, for: .navigationBar)
        .scrollContentBackground(.hidden)
        .background(BealeTheme.panel)
    }
}

private struct RemoteSessionView: View {
    let workspace: AppServerWorkspace
    let session: AppServerWorkspaceSession
    @ObservedObject var model: AppModel
    @State private var steeringInstruction = ""
    @State private var isShowingSubagents = false

    private var currentSession: AppServerWorkspaceSession {
        model.currentSession(session, in: workspace)
    }

    private var messages: [AppServerTranscriptMessage] {
        model.transcript(for: session)
    }

    var body: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 16) {
                    SessionPromptView(prompt: currentSession.prompt)
                        .id("session-prompt")

                    if model.isLoadingTranscript(for: session), messages.isEmpty {
                        HStack(spacing: 10) {
                            ProgressView()
                            Text("Loading commentary")
                                .foregroundStyle(BealeTheme.muted)
                        }
                        .frame(maxWidth: .infinity)
                        .padding(.vertical, 28)
                    }

                    ForEach(messages) { message in
                        TranscriptMessageView(message: message)
                            .id(message.id)
                    }

                    if messages.isEmpty, !model.isLoadingTranscript(for: session) {
                        Text(currentSession.status == "active" ? "Waiting for commentary…" : "No commentary was recorded.")
                            .font(.subheadline)
                            .foregroundStyle(BealeTheme.muted)
                            .frame(maxWidth: .infinity)
                            .padding(.vertical, 28)
                    }

                    if let error = model.transcriptError(for: session) {
                        Label(error, systemImage: "exclamationmark.triangle")
                            .font(.caption)
                            .foregroundStyle(.orange)
                            .frame(maxWidth: .infinity, alignment: .leading)
                    }

                    Color.clear
                        .frame(height: 1)
                        .id("session-bottom")
                }
                .padding(16)
                .frame(maxWidth: 760)
                .frame(maxWidth: .infinity)
            }
            .background(BealeTheme.panel)
            .refreshable {
                await model.refreshSession(session, in: workspace)
            }
            .onChange(of: messages.last?.id) { _, _ in
                withAnimation {
                    proxy.scrollTo("session-bottom", anchor: .bottom)
                }
            }
        }
        .navigationTitle(currentSession.displayTitle)
        .navigationBarTitleDisplayMode(.inline)
        .toolbarBackground(BealeTheme.chrome, for: .navigationBar)
        .toolbarBackground(.visible, for: .navigationBar)
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                Button {
                    isShowingSubagents = true
                } label: {
                    SessionStatusView(status: currentSession.status)
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Session status: \(currentSession.status). Show subagents.")
            }
        }
        .sheet(isPresented: $isShowingSubagents) {
            SessionSubagentsView(
                workspace: workspace,
                session: currentSession,
                model: model
            )
        }
        .safeAreaInset(edge: .bottom, spacing: 0) {
            if currentSession.status == "active" {
                if let approval = model.pendingApproval(for: session) {
                    SessionApprovalComposer(
                        approval: approval,
                        isSending: model.isSendingApproval(for: session),
                        isStopping: model.isStopping(session),
                        error: model.approvalError(for: session) ?? model.stopError(for: session),
                        decide: { decision in
                            await model.resolveApproval(
                                approval,
                                decision: decision,
                                for: session,
                                in: workspace
                            )
                        },
                        stop: { await model.stopSession(session, in: workspace) }
                    )
                } else {
                    SessionSteeringComposer(
                        instruction: $steeringInstruction,
                        isSending: model.isSendingSteering(for: session),
                        isStopping: model.isStopping(session),
                        error: model.steeringError(for: session) ?? model.stopError(for: session),
                        send: {
                            let submitted = steeringInstruction
                            if await model.sendSteering(submitted, to: session, in: workspace) {
                                steeringInstruction = ""
                            }
                        },
                        stop: { await model.stopSession(session, in: workspace) }
                    )
                }
            }
        }
        .task(id: session.id) {
            await model.followSession(session, in: workspace)
        }
    }
}

private struct SessionApprovalComposer: View {
    let approval: AppServerPendingApproval
    let isSending: Bool
    let isStopping: Bool
    let error: String?
    let decide: (String) async -> Void
    let stop: () async -> Bool

    private var grantsSession: Bool {
        approval.isComputerUse && approval.permissionMode == "once_per_session" && approval.targetBinary != nil
    }

    private var title: String {
        if grantsSession { return "Allow \(approval.targetBinary!) for this session?" }
        if approval.isComputerUse { return "Approve this computer action?" }
        return approval.approvalKind == "auto_review_override"
            ? "Approve this command once?"
            : "Approve this shell command?"
    }

    private var detail: String {
        if grantsSession { return "Allow this and later computer actions targeting \(approval.targetBinary!)." }
        if approval.isComputerUse {
            return "\(approval.toolName ?? "act") in \(approval.targetBinary ?? "the target application")."
        }
        if let reason = approval.reviewReason, !reason.isEmpty { return reason }
        return approval.commandLine ?? "The session is waiting for your review."
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            if let error {
                Text(error)
                    .font(.caption)
                    .foregroundStyle(.orange)
            }
            Text(title)
                .font(.subheadline.weight(.semibold))
                .foregroundStyle(BealeTheme.text)
            Text(detail)
                .font(.caption)
                .foregroundStyle(BealeTheme.muted)
                .lineLimit(3)
            HStack(spacing: 10) {
                Button(approval.approvalKind == "auto_review_override" ? "Keep Blocked" : "Deny") {
                    Task { await decide("denied") }
                }
                .buttonStyle(.bordered)
                .disabled(isSending || isStopping)

                Button(grantsSession ? "Allow for Session" : approval.isComputerUse ? "Approve" : "Approve Once") {
                    Task { await decide("approved") }
                }
                .buttonStyle(.borderedProminent)
                .disabled(isSending || isStopping)

                Spacer(minLength: 0)

                Button {
                    Task { _ = await stop() }
                } label: {
                    if isStopping {
                        ProgressView()
                            .frame(width: 20, height: 20)
                    } else {
                        Image(systemName: "square.fill")
                            .font(.system(size: 10, weight: .bold))
                            .frame(width: 20, height: 20)
                    }
                }
                .buttonStyle(.borderedProminent)
                .buttonBorderShape(.circle)
                .tint(.red)
                .disabled(isSending || isStopping)
                .accessibilityLabel("Stop session")
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.horizontal, 14)
        .padding(.vertical, 12)
        .background(.ultraThinMaterial)
        .accessibilityElement(children: .contain)
        .accessibilityLabel(title)
    }
}

private struct SessionSteeringComposer: View {
    @Binding var instruction: String
    let isSending: Bool
    let isStopping: Bool
    let error: String?
    let send: () async -> Void
    let stop: () async -> Bool

    private var hasInstruction: Bool {
        !instruction.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    private var canSend: Bool {
        hasInstruction && !isSending && !isStopping
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 7) {
            if let error {
                Text(error)
                    .font(.caption)
                    .foregroundStyle(.orange)
            }
            HStack(alignment: .bottom, spacing: 10) {
                TextField("Message this session", text: $instruction, axis: .vertical)
                    .lineLimit(1...5)
                    .textFieldStyle(.plain)
                    .padding(.horizontal, 12)
                    .padding(.vertical, 10)
                    .background(BealeTheme.panel, in: RoundedRectangle(cornerRadius: 12))

                Button {
                    Task {
                        if hasInstruction {
                            await send()
                        } else {
                            _ = await stop()
                        }
                    }
                } label: {
                    if isSending || isStopping {
                        ProgressView()
                            .frame(width: 20, height: 20)
                    } else if hasInstruction {
                        Image(systemName: "arrow.up")
                            .font(.body.weight(.bold))
                            .frame(width: 20, height: 20)
                    } else {
                        Image(systemName: "square.fill")
                            .font(.system(size: 10, weight: .bold))
                            .frame(width: 20, height: 20)
                    }
                }
                .buttonStyle(.borderedProminent)
                .buttonBorderShape(.circle)
                .tint(hasInstruction ? BealeTheme.text : .red)
                .disabled(hasInstruction ? !canSend : isSending || isStopping)
                .accessibilityLabel(hasInstruction ? "Send message" : "Stop session")
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 10)
        .background(.ultraThinMaterial)
    }
}

private struct SessionPromptView: View {
    let prompt: String

    var body: some View {
        HStack {
            Spacer(minLength: 44)
            Text(markdown: prompt.isEmpty ? "No prompt was recorded." : prompt)
                .foregroundStyle(BealeTheme.text)
                .textSelection(.enabled)
                .padding(.horizontal, 13)
                .padding(.vertical, 10)
                .background(BealeTheme.chrome, in: RoundedRectangle(cornerRadius: 14))
        }
        .frame(maxWidth: .infinity)
        .accessibilityElement(children: .combine)
        .accessibilityLabel("Session prompt: \(prompt)")
    }
}

private struct TranscriptMessageView: View {
    let message: AppServerTranscriptMessage

    private var isUser: Bool {
        message.role == "user"
    }

    private var isThought: Bool {
        message.source == "openai_reasoning_summary"
    }

    private var isTool: Bool {
        message.source == "app_server_tool_summary"
    }

    private var toolIcon: String {
        switch message.metadata?.toolName {
        case "shell.run", "experiment.run": "terminal"
        case "file.read": "wrench"
        case "history.search", "memory.get", "memory.search": "internaldrive"
        default: "wrench.and.screwdriver"
        }
    }

    private var thoughtLines: [String] {
        let lines = message.contentMarkdown
            .replacingOccurrences(of: "\r\n", with: "\n")
            .replacingOccurrences(of: "\r", with: "\n")
            .split(separator: "\n")
            .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
            .filter { !$0.isEmpty }
        return lines.isEmpty ? [message.contentMarkdown] : lines
    }

    @ViewBuilder
    var body: some View {
        if isUser {
            HStack {
                Spacer(minLength: 44)
                Text(markdown: message.contentMarkdown)
                    .foregroundStyle(BealeTheme.text)
                    .textSelection(.enabled)
                    .padding(.horizontal, 13)
                    .padding(.vertical, 10)
                    .background(BealeTheme.chrome, in: RoundedRectangle(cornerRadius: 14))
            }
            .frame(maxWidth: .infinity)
            .accessibilityElement(children: .combine)
            .accessibilityLabel("You: \(message.contentMarkdown)")
        } else if isThought {
            VStack(alignment: .leading, spacing: 14) {
                ForEach(Array(thoughtLines.enumerated()), id: \.offset) { _, line in
                    HStack(alignment: .top, spacing: 10) {
                        Image("ReasoningIcon")
                            .renderingMode(.template)
                            .resizable()
                            .scaledToFit()
                            .frame(width: 16, height: 16)
                            .frame(width: 18, height: 20, alignment: .center)
                            .accessibilityHidden(true)
                        Text(markdown: line)
                            .textSelection(.enabled)
                            .frame(maxWidth: .infinity, alignment: .leading)
                    }
                    .foregroundStyle(BealeTheme.muted)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .accessibilityElement(children: .combine)
            .accessibilityLabel("Thought: \(message.contentMarkdown)")
        } else if isTool {
            HStack(alignment: .top, spacing: 10) {
                Image(systemName: toolIcon)
                    .font(.system(size: 15))
                    .frame(width: 18, height: 20, alignment: .center)
                    .accessibilityHidden(true)
                Text(message.contentMarkdown)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
            .foregroundStyle(BealeTheme.muted)
            .frame(maxWidth: .infinity, alignment: .leading)
            .accessibilityElement(children: .combine)
            .accessibilityLabel("Tool: \(message.contentMarkdown)")
        } else if message.role == "system" {
            HStack(alignment: .top, spacing: 10) {
                Image(systemName: "info.circle")
                    .font(.system(size: 16))
                    .frame(width: 18, height: 20, alignment: .center)
                    .accessibilityHidden(true)
                Text(markdown: message.contentMarkdown)
                    .textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
            .foregroundStyle(BealeTheme.muted)
            .frame(maxWidth: .infinity, alignment: .leading)
            .accessibilityElement(children: .combine)
        } else {
            Text(markdown: message.contentMarkdown)
                .foregroundStyle(BealeTheme.text)
                .textSelection(.enabled)
                .frame(maxWidth: .infinity, alignment: .leading)
                .accessibilityElement(children: .combine)
                .accessibilityLabel(
                    "\(message.phase == "final_answer" ? "Response" : "Commentary"): \(message.contentMarkdown)"
                )
        }
    }
}

private struct SessionStatusView: View {
    let status: String

    var body: some View {
        ZStack {
            Circle()
                .fill(BealeTheme.panel)
                .frame(width: 28, height: 28)
            if status == "active" {
                ProgressView()
                    .controlSize(.small)
            } else {
                Circle()
                    .fill(statusColor)
                    .frame(width: 9, height: 9)
            }
        }
        .frame(width: 32, height: 32)
        .contentShape(Circle())
        .accessibilityLabel("Session status: \(status)")
    }

    private var statusColor: Color {
        switch status {
        case "completed": .green
        case "failed", "stopped": .red
        case "blocked", "paused": .orange
        default: BealeTheme.muted
        }
    }
}

private struct SessionSubagentsView: View {
    let workspace: AppServerWorkspace
    let session: AppServerWorkspaceSession
    @ObservedObject var model: AppModel
    @Environment(\.dismiss) private var dismiss

    private var currentSession: AppServerWorkspaceSession {
        model.currentSession(session, in: workspace)
    }

    private var subagents: [AppServerSubagentSummary] {
        model.subagents(for: currentSession)
    }

    private var active: [AppServerSubagentSummary] {
        subagents.filter(\.isActive)
    }

    private var completed: [AppServerSubagentSummary] {
        subagents.filter { !$0.isActive }
    }

    var body: some View {
        NavigationStack {
            List {
                if model.isLoadingCollaboration(for: session), subagents.isEmpty {
                    HStack {
                        Spacer()
                        ProgressView("Loading subagents…")
                        Spacer()
                    }
                }

                if let error = model.collaborationError(for: session) {
                    Label(error, systemImage: "exclamationmark.triangle")
                        .font(.caption)
                        .foregroundStyle(.orange)
                }

                SubagentListSection(
                    title: "Active",
                    agents: active,
                    workspace: workspace,
                    session: currentSession,
                    model: model
                )
                SubagentListSection(
                    title: "Completed",
                    agents: completed,
                    workspace: workspace,
                    session: currentSession,
                    model: model
                )
            }
            .navigationTitle("Subagents")
            .navigationBarTitleDisplayMode(.inline)
            .toolbarBackground(BealeTheme.chrome, for: .navigationBar)
            .toolbarBackground(.visible, for: .navigationBar)
            .scrollContentBackground(.hidden)
            .background(BealeTheme.panel)
            .refreshable {
                await model.loadCollaboration(for: session, in: workspace, force: true)
                await model.loadTranscript(for: session, in: workspace, force: true)
            }
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Done") { dismiss() }
                }
            }
        }
        .preferredColorScheme(.dark)
        .task(id: session.id) {
            await model.loadCollaboration(for: session, in: workspace)
        }
    }
}

private struct SubagentListSection: View {
    let title: String
    let agents: [AppServerSubagentSummary]
    let workspace: AppServerWorkspace
    let session: AppServerWorkspaceSession
    @ObservedObject var model: AppModel

    var body: some View {
        Section("\(agents.count) \(title)") {
            if agents.isEmpty {
                Text(title == "Active" ? "No active subagents right now." : "No completed subagents yet.")
                    .font(.subheadline)
                    .foregroundStyle(BealeTheme.muted)
            } else {
                ForEach(agents) { agent in
                    NavigationLink {
                        RemoteSubagentCommentaryView(
                            workspace: workspace,
                            session: session,
                            agent: agent,
                            model: model
                        )
                    } label: {
                        SubagentListRow(
                            agent: agent,
                            providerCatalog: model.providerCatalog
                        )
                    }
                }
            }
        }
    }
}

private struct SubagentListRow: View {
    let agent: AppServerSubagentSummary
    let providerCatalog: AppServerProviderCatalog?

    private var providerIdentity: String {
        let provider = providerCatalog?.providers.first { $0.providerId == agent.provider }
        let providerName = provider?.providerName ?? agent.provider ?? "Unknown provider"
        guard let modelId = agent.model else { return providerName }
        let modelName = provider?.models.first { $0.id == modelId }?.name ?? modelId
        return "\(providerName) · \(modelName)"
    }

    private var channelLabel: String {
        guard let channel = agent.channelName?.trimmingCharacters(in: .whitespacesAndNewlines),
              !channel.isEmpty else {
            return "No Channels"
        }
        return "#\(channel.trimmingCharacters(in: CharacterSet(charactersIn: "#")))"
    }

    private var createdAt: Date? {
        ISO8601DateFormatter().date(from: agent.createdAt)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 7) {
            HStack(spacing: 7) {
                SubagentStatusIndicator(status: agent.status)
                Text(agent.displayName)
                    .font(.subheadline.weight(.semibold))
                    .foregroundStyle(BealeTheme.text)
                Spacer()
                if let createdAt {
                    Text(createdAt, style: .relative)
                        .font(.caption2)
                        .foregroundStyle(BealeTheme.muted)
                }
            }

            Text(agent.latestMessage.isEmpty ? "No message yet." : agent.latestMessage)
                .font(.subheadline)
                .foregroundStyle(BealeTheme.muted)
                .lineLimit(2)

            HStack(spacing: 8) {
                Text(channelLabel)
                    .lineLimit(1)
                Spacer()
                Image(systemName: "cpu")
                    .font(.caption)
                Text(providerIdentity)
                    .lineLimit(1)
            }
            .font(.caption2)
            .foregroundStyle(BealeTheme.muted)
        }
        .padding(.vertical, 3)
        .accessibilityElement(children: .combine)
        .accessibilityLabel("\(agent.displayName), \(agent.status), \(agent.latestMessage.isEmpty ? "No message yet" : agent.latestMessage)")
    }
}

private struct SubagentStatusIndicator: View {
    let status: String

    var body: some View {
        if status == "pending" || status == "running" {
            ProgressView()
                .controlSize(.small)
                .frame(width: 16, height: 16)
                .accessibilityLabel(status.capitalized)
        } else {
            Image(systemName: "cpu")
                .font(.system(size: 14, weight: .semibold))
                .foregroundStyle(status == "completed" ? .green : .red)
                .frame(width: 16, height: 16)
                .accessibilityLabel(status.capitalized)
        }
    }
}

private struct RemoteSubagentCommentaryView: View {
    let workspace: AppServerWorkspace
    let session: AppServerWorkspaceSession
    let agent: AppServerSubagentSummary
    @ObservedObject var model: AppModel

    private var messages: [AppServerTranscriptMessage] {
        model.transcript(for: session, subagentPath: agent.path)
    }

    var body: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 16) {
                    ForEach(messages) { message in
                        TranscriptMessageView(message: message)
                            .id(message.id)
                    }

                    if messages.isEmpty, !model.isLoadingTranscript(for: session) {
                        Text(agent.isActive ? "Waiting for commentary…" : "No commentary was recorded.")
                            .font(.subheadline)
                            .foregroundStyle(BealeTheme.muted)
                            .frame(maxWidth: .infinity)
                            .padding(.vertical, 28)
                    }

                    Color.clear
                        .frame(height: 1)
                        .id("subagent-bottom")
                }
                .padding(16)
                .frame(maxWidth: 760)
                .frame(maxWidth: .infinity)
            }
            .background(BealeTheme.panel)
            .refreshable {
                await model.loadTranscript(for: session, in: workspace, force: true)
                await model.loadCollaboration(for: session, in: workspace, force: true)
            }
            .onChange(of: messages.last?.id) { _, _ in
                withAnimation {
                    proxy.scrollTo("subagent-bottom", anchor: .bottom)
                }
            }
        }
        .navigationTitle(agent.displayName)
        .navigationBarTitleDisplayMode(.inline)
        .toolbarBackground(BealeTheme.chrome, for: .navigationBar)
        .toolbarBackground(.visible, for: .navigationBar)
    }
}

private struct RemoteSessionRow: View {
    let session: AppServerWorkspaceSession

    var body: some View {
        Label {
            VStack(alignment: .leading, spacing: 3) {
                Text(session.displayTitle)
                    .foregroundStyle(BealeTheme.text)
                Text(session.status.capitalized)
                    .font(.caption)
                    .foregroundStyle(BealeTheme.muted)
            }
        } icon: {
            Image(systemName: "clock.arrow.circlepath")
                .foregroundStyle(BealeTheme.muted)
        }
        .padding(.leading, 12)
        .accessibilityElement(children: .combine)
    }
}

private extension AppServerWorkspaceSession {
    var displayTitle: String {
        let trimmedTitle = title.trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmedTitle.isEmpty ? "Untitled Session" : trimmedTitle
    }
}

private extension AppServerSubagentSummary {
    var displayName: String {
        name
            .replacingOccurrences(of: "_", with: " ")
            .split(separator: " ")
            .map { word in
                guard let first = word.first else { return "" }
                return String(first).uppercased() + word.dropFirst()
            }
            .joined(separator: " ")
    }
}

private extension Text {
    init(markdown: String) {
        let options = AttributedString.MarkdownParsingOptions(interpretedSyntax: .inlineOnlyPreservingWhitespace)
        self.init((try? AttributedString(markdown: markdown, options: options)) ?? AttributedString(markdown))
    }
}

private enum BealeSettingsTab: String, CaseIterable, Identifiable {
    case general = "General"
    case connections = "Connections"

    var id: String { rawValue }
}

private struct BealeSettingsView: View {
    @ObservedObject var model: AppModel
    @State private var selectedTab: BealeSettingsTab = .general

    var body: some View {
        VStack(spacing: 0) {
            Picker("Settings View", selection: $selectedTab) {
                ForEach(BealeSettingsTab.allCases) { tab in
                    Text(tab.rawValue).tag(tab)
                }
            }
            .pickerStyle(.segmented)
            .padding(.horizontal, 16)
            .padding(.vertical, 12)
            .background(BealeTheme.chrome)

            Group {
                switch selectedTab {
                case .general:
                    GeneralSettingsView(model: model)
                case .connections:
                    ConnectionsSettingsView(model: model)
                }
            }
        }
        .navigationTitle("Settings")
        .navigationBarTitleDisplayMode(.inline)
        .toolbarBackground(BealeTheme.chrome, for: .navigationBar)
        .toolbarBackground(.visible, for: .navigationBar)
        .background(BealeTheme.panel.ignoresSafeArea())
    }
}

private struct GeneralSettingsView: View {
    @ObservedObject var model: AppModel

    var body: some View {
        Form {
            Section {
                Toggle(
                    "Research Attention",
                    isOn: Binding(
                        get: { model.memoryNotificationsEnabled },
                        set: { enabled in
                            Task { await model.setMemoryNotificationsEnabled(enabled) }
                        }
                    )
                )

                if model.memoryNotificationAuthorization == .denied {
                    Button("Open iOS Notification Settings") {
                        guard let url = URL(string: UIApplication.openSettingsURLString) else { return }
                        UIApplication.shared.open(url)
                    }
                }
            } header: {
                Text("Notifications")
            } footer: {
                Text("Alerts are sent only for Medium or High findings that become Observed, Reproduced, or Verified. Background checks are scheduled by iOS and may not run immediately.")
            }
        }
        .scrollContentBackground(.hidden)
        .background(BealeTheme.panel)
    }
}

private struct ConnectionsSettingsView: View {
    @ObservedObject var model: AppModel

    var body: some View {
        List {
            Section("Connections") {
                ForEach(model.connections) { connection in
                    NavigationLink {
                        ConnectionView(model: model, connection: connection)
                            .navigationTitle(connection.name)
                            .navigationBarTitleDisplayMode(.inline)
                    } label: {
                        ConnectionSettingsRow(connection: connection)
                    }
                }

                NavigationLink {
                    ConnectionView(model: model)
                        .navigationTitle("Add Connection")
                        .navigationBarTitleDisplayMode(.inline)
                } label: {
                    Label("Add Connection", systemImage: "plus.circle")
                }
            }
        }
        .scrollContentBackground(.hidden)
        .background(BealeTheme.panel)
    }
}

private struct ConnectionChooserView: View {
    @ObservedObject var model: AppModel

    var body: some View {
        List {
            Section {
                if model.connections.isEmpty {
                    ContentUnavailableView(
                        "No Connections",
                        systemImage: "network.slash",
                        description: Text("Add this Mac's Beale app-server to get started.")
                    )
                    .listRowBackground(BealeTheme.panel)
                } else {
                    ForEach(model.connections) { connection in
                        Button {
                            Task { await model.connect(to: connection) }
                        } label: {
                            HStack(spacing: 12) {
                                ConnectionSettingsRow(connection: connection)
                                Spacer(minLength: 8)
                                if connection.status == "Connecting" {
                                    ProgressView()
                                } else {
                                    Image(systemName: "chevron.right")
                                        .font(.caption.weight(.semibold))
                                        .foregroundStyle(BealeTheme.muted)
                                }
                            }
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                        .disabled(model.connectionState == .connecting)
                    }
                }
            } header: {
                Text("Available Connections")
            } footer: {
                if !model.connections.isEmpty {
                    Text("Choose a saved Beale app-server to connect.")
                }
            }

            if case .failed(let message) = model.connectionState {
                Section {
                    Label(message, systemImage: "exclamationmark.triangle.fill")
                        .foregroundStyle(.red)
                }
            }

            Section {
                NavigationLink {
                    ConnectionView(model: model)
                        .navigationTitle("Add Connection")
                        .navigationBarTitleDisplayMode(.inline)
                } label: {
                    Label("Add Connection", systemImage: "plus.circle")
                }
            }
        }
        .scrollContentBackground(.hidden)
        .background(BealeTheme.panel)
    }
}

private struct ConnectionSettingsRow: View {
    let connection: AppConnection

    var body: some View {
        Label {
            VStack(alignment: .leading, spacing: 3) {
                Text(connection.name)
                    .foregroundStyle(BealeTheme.text)
                Text(connection.serverURL)
                    .font(.caption)
                    .foregroundStyle(BealeTheme.muted)
                    .lineLimit(1)
                Text(connection.status)
                    .font(.caption2)
                    .foregroundStyle(connection.isActive ? .green : BealeTheme.muted)
            }
        } icon: {
            Image(systemName: "network")
                .foregroundStyle(connection.isActive ? .green : BealeTheme.muted)
        }
        .accessibilityElement(children: .combine)
    }
}

private struct ConnectionView: View {
    @ObservedObject var model: AppModel
    private let connection: AppConnection?
    @State private var serverURL: String
    @State private var operatorToken: String
    @State private var showingScanner = false
    @State private var scannerError: String?
    @Environment(\.dismiss) private var dismiss

    init(model: AppModel, connection: AppConnection? = nil) {
        self.model = model
        self.connection = connection
        _serverURL = State(initialValue: connection?.serverURL ?? "")
        _operatorToken = State(initialValue: connection.map { model.operatorToken(for: $0) } ?? "")
    }

    var body: some View {
        Form {
            Section {
                TextField(
                    "https://your-mac.your-tailnet.ts.net",
                    text: $serverURL,
                    prompt: Text("https://your-mac.your-tailnet.ts.net")
                )
                .textInputAutocapitalization(.never)
                .keyboardType(.URL)
                .autocorrectionDisabled()

                SecureField("Operator token", text: $operatorToken)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
            } header: {
                Text("Tailscale Serve")
            } footer: {
                Text("Scan the code from the app-server tray menu, or enter the HTTPS Serve URL and operator token manually.")
            }

            Section {
                Button {
                    scannerError = nil
                    showingScanner = true
                } label: {
                    Label("Scan QR Code", systemImage: "qrcode.viewfinder")
                }
            }

            if case .failed(let message) = model.connectionState {
                Section {
                    Label(message, systemImage: "exclamationmark.triangle.fill")
                        .foregroundStyle(.red)
                }
            }

            Section {
                Button {
                    Task {
                        await model.configureAndConnect(
                            serverURL: serverURL,
                            operatorToken: operatorToken,
                            replacing: connection?.id
                        )
                    }
                } label: {
                    HStack {
                        Spacer()
                        if model.connectionState == .connecting {
                            ProgressView()
                        } else {
                            Text("Connect")
                        }
                        Spacer()
                    }
                }
                .disabled(
                    serverURL.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                        || operatorToken.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                        || model.connectionState == .connecting
                )

                if connection != nil {
                    Button("Forget Connection", role: .destructive) {
                        guard let connection else { return }
                        model.forgetConnection(connection)
                        dismiss()
                    }
                }
            }
        }
        .scrollContentBackground(.hidden)
        .background(BealeTheme.panel)
        .sheet(isPresented: $showingScanner) {
            NavigationStack {
                QRCodeScannerView(
                    onCode: applyScannedCode,
                    onFailure: { message in
                        showingScanner = false
                        scannerError = message
                    }
                )
                .ignoresSafeArea(edges: .bottom)
                .navigationTitle("Scan App Server")
                .navigationBarTitleDisplayMode(.inline)
                .toolbarBackground(BealeTheme.chrome, for: .navigationBar)
                .toolbarBackground(.visible, for: .navigationBar)
                .toolbar {
                    ToolbarItem(placement: .cancellationAction) {
                        Button("Cancel") { showingScanner = false }
                    }
                }
            }
            .preferredColorScheme(.dark)
        }
        .alert("QR Code Not Accepted", isPresented: Binding(
            get: { scannerError != nil },
            set: { presented in
                if !presented { scannerError = nil }
            }
        )) {
            Button("OK", role: .cancel) {}
        } message: {
            Text(scannerError ?? "The QR code could not be read.")
        }
    }

    private func applyScannedCode(_ value: String) {
        do {
            let payload = try AppServerPairingPayload(scannedValue: value)
            serverURL = payload.serverURL
            operatorToken = payload.operatorToken
            showingScanner = false
            Task {
                await model.configureAndConnect(
                    serverURL: serverURL,
                    operatorToken: operatorToken,
                    replacing: connection?.id
                )
            }
        } catch {
            showingScanner = false
            scannerError = error.localizedDescription
        }
    }
}

private extension String {
    var claimClassificationLabel: String {
        (split(separator: ".").last.map(String.init) ?? self).memoryTypeLabel
    }

    var memoryTypeLabel: String {
        replacingOccurrences(of: "_", with: " ")
            .replacingOccurrences(of: "-", with: " ")
            .split(separator: " ")
            .map { $0.capitalized }
            .joined(separator: " ")
    }
}
