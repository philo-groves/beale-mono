import Foundation
import Security

protocol OperatorTokenStore: Sendable {
    func loadOperatorToken(connectionID: String) -> String
    func saveOperatorToken(_ token: String, connectionID: String) throws
    func deleteOperatorToken(connectionID: String)
    func loadLegacyOperatorToken() -> String
    func deleteLegacyOperatorToken()
}

struct KeychainStore: OperatorTokenStore, Sendable {
    private let service = "com.beale.ios.app-server"
    private let legacyAccount = "operator-token"

    func loadOperatorToken(connectionID: String) -> String {
        loadOperatorToken(account: account(connectionID: connectionID))
    }

    func saveOperatorToken(_ token: String, connectionID: String) throws {
        try saveOperatorToken(token, account: account(connectionID: connectionID))
    }

    func deleteOperatorToken(connectionID: String) {
        deleteOperatorToken(account: account(connectionID: connectionID))
    }

    func loadLegacyOperatorToken() -> String {
        loadOperatorToken(account: legacyAccount)
    }

    func deleteLegacyOperatorToken() {
        deleteOperatorToken(account: legacyAccount)
    }

    private func account(connectionID: String) -> String {
        "operator-token.\(connectionID)"
    }

    private func loadOperatorToken(account: String) -> String {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
            kSecMatchLimit as String: kSecMatchLimitOne,
            kSecReturnData as String: true
        ]
        var result: CFTypeRef?
        guard SecItemCopyMatching(query as CFDictionary, &result) == errSecSuccess,
              let data = result as? Data,
              let token = String(data: data, encoding: .utf8) else {
            return ""
        }
        return token
    }

    private func saveOperatorToken(_ token: String, account: String) throws {
        let data = Data(token.utf8)
        let identity: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account
        ]
        let attributes: [String: Any] = [
            kSecValueData as String: data,
            kSecAttrAccessible as String: kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
        ]
        let status = SecItemUpdate(identity as CFDictionary, attributes as CFDictionary)
        if status == errSecItemNotFound {
            var insertion = identity
            insertion.merge(attributes) { _, new in new }
            let insertionStatus = SecItemAdd(insertion as CFDictionary, nil)
            guard insertionStatus == errSecSuccess else {
                throw KeychainError.status(insertionStatus)
            }
        } else if status != errSecSuccess {
            throw KeychainError.status(status)
        }
    }

    private func deleteOperatorToken(account: String) {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account
        ]
        SecItemDelete(query as CFDictionary)
    }
}

private enum KeychainError: LocalizedError {
    case status(OSStatus)

    var errorDescription: String? {
        switch self {
        case .status(let status):
            return "The operator token could not be saved to Keychain (\(status))."
        }
    }
}
