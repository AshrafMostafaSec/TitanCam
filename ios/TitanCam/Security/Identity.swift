import Foundation
import CryptoKit
import Security
import X509
import SwiftASN1
final class DeviceIdentity {
    let signing: Curve25519.Signing.PrivateKey
    let tls: SecIdentity
    let certificate: Data
    var publicKey: Data { signing.publicKey.rawRepresentation }
    var fingerprint: String { Data(SHA256.hash(data: certificate)).hex }
    init() throws {
        let seed = try Keychain.loadOrCreate("signing", create: { Curve25519.Signing.PrivateKey().rawRepresentation })
        signing = try Curve25519.Signing.PrivateKey(rawRepresentation: seed)
        let tlsSeed = try Keychain.loadOrCreate("tls-p256", create: { P256.Signing.PrivateKey().rawRepresentation })
        let p256 = try P256.Signing.PrivateKey(rawRepresentation: tlsSeed)
        let certBytes = try Keychain.loadOrCreate("tls-certificate") {
            let key = Certificate.PrivateKey(p256)
            let name = try DistinguishedName { CommonName("TitanCam iPhone") }
            let cert = try Certificate(version: .v3, serialNumber: .init(bytes: Array(SHA256.hash(data: p256.publicKey.rawRepresentation).prefix(16))), publicKey: key.publicKey, notValidBefore: Date().addingTimeInterval(-3600), notValidAfter: Date().addingTimeInterval(365 * 24 * 3600), issuer: name, subject: name, extensions: Certificate.Extensions {}, issuerPrivateKey: key)
            var serializer = DER.Serializer(); try serializer.serialize(cert); return Data(serializer.serializedBytes)
        }
        certificate = certBytes
        var error: Unmanaged<CFError>?
        let key = SecKeyCreateWithData(p256.x963Representation as CFData, [kSecAttrKeyType: kSecAttrKeyTypeECSECPrimeRandom, kSecAttrKeyClass: kSecAttrKeyClassPrivate, kSecAttrKeySizeInBits: 256] as CFDictionary, &error)
        guard let key else { throw CameraError.unavailable("Could not create TLS private key") }
        let keyTag = Data("titancam.tls.private".utf8)
        let status = SecItemAdd([kSecClass: kSecClassKey, kSecAttrApplicationTag: keyTag, kSecValueRef: key, kSecAttrAccessible: kSecAttrAccessibleWhenUnlockedThisDeviceOnly] as CFDictionary, nil)
        guard status == errSecSuccess || status == errSecDuplicateItem else { throw CameraError.unavailable("Keychain private key error \(status)") }
        guard let cert = SecCertificateCreateWithData(nil, certBytes as CFData) else { throw CameraError.unavailable("Invalid TLS certificate") }
        let add = SecItemAdd([kSecClass: kSecClassCertificate, kSecValueRef: cert, kSecAttrLabel: "TitanCam TLS"] as CFDictionary, nil)
        guard add == errSecSuccess || add == errSecDuplicateItem else { throw CameraError.unavailable("Keychain certificate error \(add)") }
        var result: CFTypeRef?
        let query: [CFString: Any] = [kSecClass: kSecClassIdentity, kSecReturnRef: true, kSecMatchLimit: kSecMatchLimitAll]
        guard SecItemCopyMatching(query as CFDictionary, &result) == errSecSuccess, let identities = result as? [SecIdentity], let identity = identities.first(where: { var c: SecCertificate?; return SecIdentityCopyCertificate($0, &c) == errSecSuccess && c.map { SecCertificateCopyData($0) as Data == certBytes } == true }) else { throw CameraError.unavailable("Could not locate TLS Keychain identity") }
        tls = identity
    }
    static func transcript(role: UInt8, phone: Data, receiver: Data, nonce: Data, session: Data) -> Data { var result = Data("TitanCam-auth-v1".utf8); result.append(role); result.append(contentsOf: SHA256.hash(data: phone)); result.append(contentsOf: SHA256.hash(data: receiver)); result.append(nonce); result.append(session); return result }
    func proof(receiver: Data, nonce: Data, session: Data) throws -> String { try signing.signature(for: Self.transcript(role: 1, phone: publicKey, receiver: receiver, nonce: nonce, session: session)).hex }
    func verify(signature: String, receiver: Data, nonce: Data, session: Data) throws { guard let sig = Data(hex: signature), let key = try? Curve25519.Signing.PublicKey(rawRepresentation: receiver), key.isValidSignature(sig, for: Self.transcript(role: 2, phone: publicKey, receiver: receiver, nonce: nonce, session: session)) else { throw CameraError.protocolViolation("Receiver identity proof rejected") } }
}
enum Keychain {
    static func load(_ name: String) -> Data? { var result: CFTypeRef?; let q: [CFString: Any] = [kSecClass: kSecClassGenericPassword, kSecAttrService: "TitanCam", kSecAttrAccount: name, kSecReturnData: true, kSecMatchLimit: kSecMatchLimitOne]; return SecItemCopyMatching(q as CFDictionary, &result) == errSecSuccess ? result as? Data : nil }
    static func loadOrCreate(_ name: String, create: () throws -> Data) throws -> Data { if let data = load(name) { return data }; let data = try create(); try save(name, data); return data }
    static func save(_ name: String, _ data: Data) throws { let q: [CFString: Any] = [kSecClass: kSecClassGenericPassword, kSecAttrService: "TitanCam", kSecAttrAccount: name]; SecItemDelete(q as CFDictionary); var add = q; add[kSecValueData] = data; add[kSecAttrAccessible] = kSecAttrAccessibleWhenUnlockedThisDeviceOnly; let status = SecItemAdd(add as CFDictionary, nil); guard status == errSecSuccess else { throw CameraError.unavailable("Keychain storage error \(status)") } }
    static func remove(_ name: String) { SecItemDelete([kSecClass: kSecClassGenericPassword, kSecAttrService: "TitanCam", kSecAttrAccount: name] as CFDictionary) }
}
