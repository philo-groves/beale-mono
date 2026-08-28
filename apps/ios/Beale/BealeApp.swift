import SwiftUI
import UserNotifications

@main
struct BealeApp: App {
    @StateObject private var model = AppModel()

    init() {
        UNUserNotificationCenter.current().delegate = BealeNotificationPresenter.shared
    }

    var body: some Scene {
        WindowGroup {
            ContentView(model: model)
                .preferredColorScheme(.dark)
        }
        .backgroundTask(.appRefresh(AppModel.memoryRefreshTaskIdentifier)) {
            await model.performBackgroundMemoryRefresh()
        }
    }
}
