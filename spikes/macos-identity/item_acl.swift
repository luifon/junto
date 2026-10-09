// Prints the access control entries of one generic-password item: which
// operations each entry covers, which applications it lists, and the
// partition list. Reads no secret. Usage: swift item_acl.swift <service>
import Foundation
import Security

func textFromHex(_ hex: String) -> String? {
    var bytes = [UInt8]()
    var index = hex.startIndex
    while index < hex.endIndex {
        guard let next = hex.index(index, offsetBy: 2, limitedBy: hex.endIndex),
              let byte = UInt8(hex[index..<next], radix: 16) else { return nil }
        bytes.append(byte)
        index = next
    }
    return String(bytes: bytes, encoding: .utf8)
}

let service = CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : ""
let query: [String: Any] = [
    kSecClass as String: kSecClassGenericPassword,
    kSecAttrService as String: service,
    kSecAttrAccount as String: "device-key",
    kSecReturnRef as String: true,
    kSecMatchLimit as String: kSecMatchLimitOne,
]
var found: CFTypeRef?
let lookupStatus = SecItemCopyMatching(query as CFDictionary, &found)
guard lookupStatus == errSecSuccess, let item = found else {
    print("lookup failed: \(lookupStatus)")
    exit(1)
}
var access: SecAccess?
let accessStatus = SecKeychainItemCopyAccess(item as! SecKeychainItem, &access)
guard accessStatus == errSecSuccess, let access else {
    print("access lookup failed: \(accessStatus)")
    exit(1)
}
var entries: CFArray?
SecAccessCopyACLList(access, &entries)
for entry in (entries as? [SecACL]) ?? [] {
    var applications: CFArray?
    var description: CFString?
    var prompt = SecKeychainPromptSelector()
    SecACLCopyContents(entry, &applications, &description, &prompt)
    let operations = (SecACLCopyAuthorizations(entry) as? [String]) ?? []
    print("entry: \(operations.joined(separator: ", "))")
    let label = (description as String?) ?? ""
    if operations.contains("ACLAuthorizationPartitionID"), let decoded = textFromHex(label) {
        let partitions = decoded.components(separatedBy: "<string>").dropFirst().map {
            $0.components(separatedBy: "</string>")[0]
        }
        print("  partitions: \(partitions)")
    } else {
        print("  description: \(label)")
    }
    if let listed = applications as? [SecTrustedApplication] {
        for application in listed {
            var data: CFData?
            SecTrustedApplicationCopyData(application, &data)
            let path = (data as Data?).flatMap { String(data: $0, encoding: .utf8) } ?? "?"
            let home = NSHomeDirectory()
            print("  application: \(path.replacingOccurrences(of: home, with: "~").trimmingCharacters(in: .controlCharacters))")
        }
    } else {
        print("  applications: any")
    }
}
