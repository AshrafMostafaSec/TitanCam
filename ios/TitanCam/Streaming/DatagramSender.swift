import Foundation
import Network
final class DatagramSender {
    private let connection: NWConnection
    private let queue: DispatchQueue
    private var video = MediaSendQueue(video: true)
    private var audio = MediaSendQueue(video: false)
    private var pacer = MediaPacer()
    private var timer: DispatchSourceTimer?
    private var binding: Data?
    private var bindingAt: UInt64 = 0
    private var inFlight = 0
    private var pacingMilliseconds = 2
    private var cache: [UInt64: (EncodedUnit, UInt64)] = [:]
    private var repairs: [(Data, UInt64)] = []
    private var repairTokens: Double = 0
    private var repairAt: UInt64 = 0
    private var bitrate = 14_000_000
    private var stopped = false
    private var ready = false
    private var lossAt: UInt64 = 0
    var dropped: (() -> Void)?
    init(host: String, port: UInt16, queue: DispatchQueue) {
        self.queue = queue
        connection = NWConnection(host: NWEndpoint.Host(host), port: NWEndpoint.Port(rawValue: port)!, using: .udp)
    }
    func start(session: Data, token: Data, ready: @escaping () -> Void, failure: @escaping (Error?) -> Void) {
        connection.stateUpdateHandler = { [weak self] state in
            guard let self, !self.stopped else { return }
            switch state {
            case .ready:
                guard !self.ready else { return }; self.ready = true
                var binding = Data("TCMB".utf8); binding.append(1); binding.append(session); binding.appendBE(UInt32(1)); binding.append(token)
                self.binding = binding; self.bindingAt = CaptureEngine.hostTime
                self.connection.send(content: binding, completion: .contentProcessed { [weak self] error in
                    guard let self, !self.stopped else { return }
                    if let error { self.stop(); failure(error) } else { ready() }
                })
                let timer = DispatchSource.makeTimerSource(queue: self.queue)
                timer.schedule(deadline: .now(), repeating: .milliseconds(self.pacingMilliseconds), leeway: .microseconds(100))
                timer.setEventHandler { [weak self] in self?.pump() }; self.timer = timer; timer.resume()
            case .failed(let error): self.stop(); failure(error)
            case .cancelled: self.stop(); failure(nil)
            default: break
            }
        }; connection.start(queue: queue)
    }
    func bound() { binding = nil }
    func configure(_ config: StreamConfig) {
        bitrate = config.bitrate
        cache = cache.filter { $0.value.0.config == config.config_id }; repairs.removeAll(keepingCapacity: true)
        pacer.configure(bitrate: config.bitrate, wireBudgetMbps: config.wifiBudgetMbps)
        pacingMilliseconds = config.profile == "saver" ? 5 : config.profile == "maximum" ? 1 : 2
        timer?.schedule(deadline: .now(), repeating: .milliseconds(pacingMilliseconds), leeway: .microseconds(100))
    }
    func enqueue(_ unit: EncodedUnit) {
        guard !stopped else { return }; let now = CaptureEngine.hostTime
        if unit.kind == 1 {
            cache = cache.filter { now >= $0.value.1 && now - $0.value.1 < 80_000_000 && $0.value.0.config == unit.config }
            if cache.count < 8 && cache.values.reduce(0, { $0 + $1.0.bytes.count }) + unit.bytes.count <= 8 * 1024 * 1024 { cache[unit.sequence] = (unit, now) }
        }
        let lost = unit.kind == 1 ? video.enqueue(unit, now: now) : audio.enqueue(unit, now: now)
        if lost { reportLoss(now: now) }
        pump()
    }
    func repair(_ body: [String: Any]) {
        let now = CaptureEngine.hostTime
        guard let text = body["sequence"] as? String, let sequence = UInt64(text), let (unit, created) = cache[sequence],
              (body["config_id"] as? NSNumber)?.uint32Value == unit.config, now >= created, now - created < 80_000_000,
              let missing = body["missing"] as? [NSNumber], missing.count <= 64 else { return }
        let count = (unit.bytes.count + 1035) / 1036
        for value in missing {
            let index = value.intValue
            guard index >= 0, index < count, repairs.count < 128 else { continue }
            let offset = index * 1036
            repairs.append((unit.packet(index: UInt16(index), count: UInt16(count), offset: offset, length: min(1036, unit.bytes.count - offset)), created + 80_000_000))
        }
        pump()
    }
    private func reportLoss(now: UInt64) {
        guard lossAt == 0 || now - lossAt >= 250_000_000 else { return }
        lossAt = now; dropped?()
    }
    private func pump() {
        guard ready, !stopped else { return }; let now = CaptureEngine.hostTime
        if let binding, now - bindingAt > 100_000_000 {
            bindingAt = now; connection.send(content: binding, completion: .contentProcessed { _ in })
        }
        if repairAt > 0 && now >= repairAt { repairTokens = min(16_384, repairTokens + Double(now - repairAt) / 1e9 * Double(bitrate) / 80) }
        repairAt = now
        repairs.removeAll { $0.1 <= now }
        pacer.advance(now: now)
        if video.expire(now: now) { reportLoss(now: now) }; _ = audio.expire(now: now)
        // Per-turn cap is no longer the stream's bitrate limit. Send completions
        // refill the bounded flight window, while both token buckets still apply.
        for _ in 0..<128 {
            guard inFlight < 32 else { return }
            if audio.first == nil, let (packet, _) = repairs.first, repairTokens >= Double(packet.count), pacer.take(bytes: packet.count) {
                repairs.removeFirst(); repairTokens -= Double(packet.count); inFlight += 1
                connection.send(content: packet, completion: .contentProcessed { [weak self] _ in guard let self, !self.stopped else { return }; self.inFlight -= 1; self.pump() })
                continue
            }
            let isAudio = audio.first != nil
            guard let current = isAudio ? audio.first : video.first else { return }
            let size = min(1_036, current.unit.bytes.count - current.offset)
            guard pacer.take(bytes: size + 64, audio: isAudio) else { return }
            let count = (current.unit.bytes.count + 1_035) / 1_036
            let packet = current.unit.packet(index: UInt16(current.offset / 1_036), count: UInt16(count), offset: current.offset, length: size)
            if isAudio { audio.consume(size) } else { video.consume(size) }
            inFlight += 1
            connection.send(content: packet, completion: .contentProcessed { [weak self] error in
                guard let self, !self.stopped else { return }; self.inFlight -= 1
                if error != nil { self.video.lose(); self.reportLoss(now: CaptureEngine.hostTime) }
                self.pump()
            })
        }
    }
    func stop() {
        guard !stopped else { return }; stopped = true
        timer?.cancel(); timer = nil; connection.stateUpdateHandler = nil
        connection.cancel(); video.lose(); audio.lose(); cache.removeAll(); repairs.removeAll(); dropped = nil
    }
    deinit { timer?.cancel(); connection.stateUpdateHandler = nil; connection.cancel() }
}
