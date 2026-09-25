// Read or store the GainSnap publisher token without putting it in argv or a file.
import Foundation
import Security

let account = "gainsnap"
let service = "org.portalsurfer.gainsnap.release-token"
let operation = CommandLine.arguments.dropFirst().first ?? ""
let query: [String: Any] = [
    kSecClass as String: kSecClassGenericPassword,
    kSecAttrAccount as String: account,
    kSecAttrService as String: service,
]

switch operation {
case "read":
    var readQuery = query
    readQuery[kSecReturnData as String] = true
    readQuery[kSecMatchLimit as String] = kSecMatchLimitOne
    var result: CFTypeRef?
    let status = SecItemCopyMatching(readQuery as CFDictionary, &result)
    guard status == errSecSuccess, let data = result as? Data else {
        fputs("GainSnap publisher token is not available in Keychain (\(status))\n", stderr)
        exit(status == errSecItemNotFound ? 3 : 1)
    }
    FileHandle.standardOutput.write(data)
case "store":
    let data = FileHandle.standardInput.readDataToEndOfFile()
    guard !data.isEmpty, data.count <= 256 else {
        fputs("publisher token input is empty or too large\n", stderr)
        exit(2)
    }
    var entry = query
    entry[kSecValueData as String] = data
    var status = SecItemAdd(entry as CFDictionary, nil)
    if status == errSecDuplicateItem {
        status = SecItemUpdate(query as CFDictionary, [kSecValueData as String: data] as CFDictionary)
    }
    guard status == errSecSuccess else {
        fputs("could not store publisher token in Keychain (\(status))\n", stderr)
        exit(1)
    }
case "delete":
    let status = SecItemDelete(query as CFDictionary)
    guard status == errSecSuccess || status == errSecItemNotFound else {
        fputs("could not delete publisher token from Keychain (\(status))\n", stderr)
        exit(1)
    }
default:
    fputs("usage: swift scripts/local_keychain.swift read|store|delete\n", stderr)
    exit(2)
}
