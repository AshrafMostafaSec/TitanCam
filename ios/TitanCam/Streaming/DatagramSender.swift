import Foundation
import Network
final class DatagramSender {
    struct Pending { let unit: EncodedUnit; var offset: Int; let created: UInt64 }
    private let connection: NWConnection
    private let queue: DispatchQueue
    private var video: [Pending] = []
    private var audio: [Pending] = []
    private var timer: DispatchSourceTimer?
    private var binding: Data?
    private var bindingAt: UInt64 = 0
    private var inFlight = 0
    private var tokens: Double = 0
    private var last: UInt64 = CaptureEngine.hostTime
    private var budget: Double = 2_000_000
    var dropped: (() -> Void)?
    init(host: String, port: UInt16, pin: String, queue: DispatchQueue) {
        self.queue = queue
        let options = NWProtocolQUIC.Options(alpn: ["titancam-media/1"]); options.isDatagram = true; options.maxDatagramFrameSize = 65_535; options.maxUDPPayloadSize = 1_200; options.idleTimeout = 5_000
        Connections.pin(options.securityProtocolOptions, fingerprint: pin, queue: queue)
        connection = NWConnection(host: NWEndpoint.Host(host), port: NWEndpoint.Port(rawValue: port)!, using: NWParameters(quic: options))
    }
    func start(session: Data, token: Data, ready: @escaping () -> Void, failure: @escaping (Error?) -> Void) {
        connection.stateUpdateHandler = { [weak self] state in guard let self else { return }; switch state { case .ready:
            var binding = Data("TCMB".utf8); binding.append(1); binding.append(session); binding.appendBE(UInt32(1)); binding.append(token)
            self.binding = binding; self.bindingAt = CaptureEngine.hostTime
            self.connection.send(content: binding, completion: .contentProcessed { error in if let error { failure(error) } else { ready() } }); self.receive()
            let timer = DispatchSource.makeTimerSource(queue: self.queue); timer.schedule(deadline: .now(), repeating: .milliseconds(1), leeway: .microseconds(100)); timer.setEventHandler { [weak self] in self?.pump() }; self.timer = timer; timer.resume()
        case .failed(let error): failure(error); case .cancelled: failure(nil); default: break } }; connection.start(queue: queue)
    }
    func bound() { binding = nil }
    func configure(_ config: StreamConfig) { budget = Double(config.bitrate + 400_000) / 8 }
    func enqueue(_ unit: EncodedUnit) { let pending = Pending(unit: unit, offset: 0, created: CaptureEngine.hostTime)
        if unit.kind == 1 { if video.count >= 2 { video.removeAll(keepingCapacity: true); dropped?(); if !unit.independent { return } }; video.append(pending) }
        else { if audio.count >= 10 { audio.removeAll(keepingCapacity: true) }; audio.append(pending) }
    }
    private func receive() { connection.receiveMessage { [weak self] _, _, _, error in if error == nil { self?.receive() } } }
    private func pump() {
        let now = CaptureEngine.hostTime; if let binding, now - bindingAt > 100_000_000 { bindingAt = now; connection.send(content: binding, completion: .contentProcessed { _ in }) }; tokens = min(budget * 0.02, tokens + Double(now - last) / 1e9 * budget); last = now
        if let old = video.first, now - old.created > 100_000_000 { video.removeAll(keepingCapacity: true); dropped?() }
        for _ in 0..<8 {
            guard inFlight < 8 else { return }; let isAudio = !audio.isEmpty; guard isAudio || !video.isEmpty else { return }
            var current = isAudio ? audio.removeFirst() : video.removeFirst(); let size = min(1_036, current.unit.bytes.count - current.offset)
            guard tokens >= Double(size + 64) else { if isAudio { audio.insert(current, at: 0) } else { video.insert(current, at: 0) }; return }
            let count = (current.unit.bytes.count + 1_035) / 1_036; guard count <= 16_384 else { dropped?(); continue }
            let packet = current.unit.packet(index: UInt16(current.offset / 1_036), count: UInt16(count), offset: current.offset, length: size); current.offset += size
            if current.offset < current.unit.bytes.count { if isAudio { audio.insert(current, at: 0) } else { video.insert(current, at: 0) } }
            tokens -= Double(packet.count); inFlight += 1; connection.send(content: packet, completion: .contentProcessed { [weak self] error in guard let self else { return }; self.inFlight -= 1; if error != nil { self.video.removeAll(); self.dropped?() } })
        }
    }
    func stop() { timer?.cancel(); timer = nil; connection.cancel(); video.removeAll(); audio.removeAll() }
    deinit { timer?.cancel(); connection.cancel() }
}
