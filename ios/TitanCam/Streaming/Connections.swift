import Foundation
import Network
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
        connection.send(content: record, completion: .contentProcessed { [weak self] error in guard let self else { return }; self.pending -= 1; if let error { self.close(error) } else if !self.closed { completion?() } }); return true
    }
    private func readLength() { connection.receive(minimumIncompleteLength: 4, maximumLength: 4) { [weak self] data, _, complete, error in guard let self else { return }; guard let data, data.count == 4, let count = data.integer(at: 0, UInt32.self), count > 0, count <= 65_536, !complete, error == nil else { self.close(error); return }; self.readBody(Int(count)) } }
    private func readBody(_ count: Int) { connection.receive(minimumIncompleteLength: count, maximumLength: count) { [weak self] data, _, complete, error in guard let self else { return }; guard let data, data.count == count, error == nil else { self.close(error); return }; self.receive?(data); if complete { self.close(nil) } else { self.readLength() } } }
    func close(_ error: Error? = nil) { guard !closed else { return }; closed = true; connection.cancel(); let callback = failure; failure = nil; receive = nil; connection.stateUpdateHandler = nil; callback?(error) }
}
enum Connections {
    static func tcp() -> NWParameters {
        let options = NWProtocolTCP.Options(); options.noDelay = true
        return NWParameters(tls: nil, tcp: options)
    }
}
