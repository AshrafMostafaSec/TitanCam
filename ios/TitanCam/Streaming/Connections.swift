import Foundation
import Network
import Security
import CryptoKit
final class FramedConnection {
    let connection: NWConnection
    let queue: DispatchQueue
    var receive: ((Data) -> Void)?
    var failure: ((Error?) -> Void)?
    private var pending = 0
    private var closed = false
    init(_ connection: NWConnection, queue: DispatchQueue) { self.connection = connection; self.queue = queue }
    func start() { connection.stateUpdateHandler = { [weak self] state in guard let self else { return }; switch state { case .ready: self.readLength(); case .failed(let error): self.close(error); case .cancelled: self.close(nil); default: break } }; connection.start(queue: queue) }
    func send(_ data: Data, media: Bool = false, completion: (() -> Void)? = nil) -> Bool {
        guard !closed, pending < (media ? 2 : 32), data.count <= (media ? 8 * 1024 * 1024 + 64 : 65_536) else { return false }
        var record = Data(); record.appendBE(UInt32(data.count)); record.append(data); pending += 1
        connection.send(content: record, completion: .contentProcessed { [weak self] error in guard let self else { return }; self.pending -= 1; if let error { self.close(error) }; completion?() }); return true
    }
    private func readLength() { connection.receive(minimumIncompleteLength: 4, maximumLength: 4) { [weak self] data, _, complete, error in guard let self else { return }; guard let data, data.count == 4, let count = data.integer(at: 0, UInt32.self), count > 0, count <= 65_536, !complete, error == nil else { self.close(error); return }; self.readBody(Int(count)) } }
    private func readBody(_ count: Int) { connection.receive(minimumIncompleteLength: count, maximumLength: count) { [weak self] data, _, complete, error in guard let self else { return }; guard let data, data.count == count, error == nil else { self.close(error); return }; self.receive?(data); if complete { self.close(nil) } else { self.readLength() } } }
    func close(_ error: Error? = nil) { guard !closed else { return }; closed = true; connection.cancel(); failure?(error) }
}
enum Connections {
    static func serverTLS(identity: DeviceIdentity, alpn: String) throws -> NWParameters { let tls = NWProtocolTLS.Options(); guard let secIdentity = sec_identity_create(identity.tls) else { throw CameraError.unavailable("TLS identity not available") }; sec_protocol_options_set_local_identity(tls.securityProtocolOptions, secIdentity); sec_protocol_options_set_min_tls_protocol_version(tls.securityProtocolOptions, .TLSv13); sec_protocol_options_add_tls_application_protocol(tls.securityProtocolOptions, alpn); let tcp = NWProtocolTCP.Options(); tcp.noDelay = true; return NWParameters(tls: tls, tcp: tcp) }
    static func pin(_ options: sec_protocol_options_t, fingerprint: String, queue: DispatchQueue) {
        sec_protocol_options_set_min_tls_protocol_version(options, .TLSv13)
        sec_protocol_options_set_verify_block(options, { _, trust, complete in
            let secTrust = sec_trust_copy_ref(trust).takeRetainedValue()
            guard let cert = SecTrustGetCertificateAtIndex(secTrust, 0) else { complete(false); return }
            let bytes = SecCertificateCopyData(cert) as Data
            guard Data(SHA256.hash(data: bytes)).hex == fingerprint else { complete(false); return }
            // A local self-issued pinned cert is the explicit anchor; trust still checks certificate validity and usage.
            SecTrustSetAnchorCertificates(secTrust, [cert] as CFArray); SecTrustSetAnchorCertificatesOnly(secTrust, true); SecTrustSetPolicies(secTrust, SecPolicyCreateBasicX509())
            complete(SecTrustEvaluateWithError(secTrust, nil))
        }, queue)
    }
    static func clientTLS(pin: String, queue: DispatchQueue) -> NWParameters { let tls = NWProtocolTLS.Options(); self.pin(tls.securityProtocolOptions, fingerprint: pin, queue: queue); sec_protocol_options_add_tls_application_protocol(tls.securityProtocolOptions, "titancam-control/1"); let tcp = NWProtocolTCP.Options(); tcp.noDelay = true; return NWParameters(tls: tls, tcp: tcp) }
}
