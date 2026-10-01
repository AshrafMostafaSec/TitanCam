import Foundation
import Network
import Security
import UIKit
final class StreamCoordinator {
    let capture = CaptureEngine()
    let queue = DispatchQueue(label: "titancam.network", qos: .userInteractive)
    var update: ((String, String, [String: Any]) -> Void)?
    private var control: FramedConnection?
    private var video: FramedConnection?
    private var audio: FramedConnection?
    private var datagrams: DatagramSender?
    private var usbVideo: USBMediaSender?
    private var usbAudio: USBMediaSender?
    private var listeners: [NWListener] = []
    private var retiringListeners: [NWListener] = []
    private var shutdownCompletions: [() -> Void] = []
    private(set) var usbBasePort: UInt16?
    private var bootstrapConnections: [NWConnection] = []
    private var usbReadyRoles = Set<String>()
    private var preparingUSB = false
    private var wifiControlPort: UInt16 = 49160
    static let usbPortCandidates: [UInt16] = [43052, 43062, 43072]
    private var negotiated = false
    private var mediaConnections = 0
    private var streaming = false
    private var configurationPending = false
    private var configurationPaused = false
    private var session = ""
    private var token = Data()
    private var wifiHost = ""
    private var mediaPort: UInt16 = 49161
    private var current = StreamConfig()
    private var heartbeat: DispatchSourceTimer?
    private var userStopped = true
    private var reconnectAttempt = 0
    private var generation = 0
    private var lastVideo = 0
    private var lastExpired = 0
    private var lastLate = 0
    private var lastMissing = 0
    private var adaptiveAt: UInt64 = 0
    private var stableSince: UInt64 = 0
    private var requestedBitrate = 14_000_000
    var usbReadyDescription: String { "Waiting for your computer. Keep the cable connected and TitanCam open on Linux." }
    init() throws {
        capture.output = { [weak self] unit in self?.queue.async { self?.send(unit) } }
        capture.failure = { [weak self] error in self?.queue.async { self?.fail(error, retry: error.hasPrefix("Capture interrupted:")) } }
    }
    func startWiFi(_ uri: String) {
        queue.async { do {
            guard let url = URLComponents(string: uri), url.scheme == "titancam", url.host == "local" else { throw CameraError.protocolViolation("Choose a compatible computer from the list") }
            let fields = Dictionary(url.queryItems?.compactMap { item in item.value.map { (item.name, $0) } } ?? [], uniquingKeysWith: { a, _ in a })
            guard let host = fields["host"], !host.isEmpty, host.utf8.count <= 253,
                  let controlPort = UInt16(fields["port"] ?? "49160"), controlPort > 0,
                  let mediaPort = UInt16(fields["media"] ?? "49161"), mediaPort > 0 else { throw CameraError.protocolViolation("Invalid computer address") }
            self.generation += 1; let expected = self.generation; self.userStopped = false
            self.update?("Connecting", "Closing the previous transport", [:])
            self.stopConnections { [weak self] in
                guard let self, self.generation == expected, !self.userStopped else { return }
                self.wifiHost = host
                self.mediaPort = mediaPort; self.wifiControlPort = controlPort
                self.reconnectAttempt = 0; self.connectWiFi()
            }
        } catch { self.fail(error.localizedDescription, retry: false) } }
    }
    private func connectWiFi() {
        let connection = NWConnection(host: NWEndpoint.Host(wifiHost), port: NWEndpoint.Port(rawValue: wifiControlPort)!, using: Connections.tcp())
        let framed = FramedConnection(connection, queue: queue); control = framed; attachControl(framed, usb: false); framed.start()
        update?("Connecting", "Connecting to your computer", [:])
    }
    func enableUSB() {
        queue.async {
            // A repeated button tap must not create another listener set on the same ports.
            if self.preparingUSB && !self.userStopped { return }
            if self.usbReadyRoles.count == 4 && self.listeners.count == 4 {
                if !self.streaming { self.update?("USB ready", self.usbReadyDescription, [:]) }
                return
            }
            self.generation += 1; let expected = self.generation; self.userStopped = false
            self.preparingUSB = true; self.update?("Preparing USB", "Preparing the cable connection", [:])
            self.stopConnections { [weak self] in
                guard let self, self.generation == expected, !self.userStopped else { return }
                self.wifiHost = ""; self.startUSBListeners(attempt: 0, generation: expected)
            }
        }
    }
    private func startUSBListeners(attempt: Int, generation expected: Int) {
        let base = Self.usbPortCandidates[attempt]; usbBasePort = base; preparingUSB = true; usbReadyRoles.removeAll()
        do {
            for (offset, role) in ["bootstrap", "control", "video", "audio"].enumerated() {
                let port = base - 1 + UInt16(offset)
                let parameters = Connections.tcp()
                parameters.allowLocalEndpointReuse = true
                let listener = try NWListener(using: parameters, on: NWEndpoint.Port(rawValue: port)!)
                listener.newConnectionHandler = { [weak self] connection in
                    guard let self, self.generation == expected, !self.userStopped else { connection.cancel(); return }
                    if role == "bootstrap" {
                        guard self.usbReadyRoles.count == 4, self.bootstrapConnections.count < 4,
                              let metadata = try? JSONSerialization.data(withJSONObject: ["version": 2, "base": Int(base)]) else { connection.cancel(); return }
                        self.bootstrapConnections.append(connection)
                        let finish = { [weak self, weak connection] in
                            guard let connection else { return }; connection.cancel(); connection.stateUpdateHandler = nil
                            self?.bootstrapConnections.removeAll { $0 === connection }
                        }
                        connection.stateUpdateHandler = { state in
                            switch state {
                            case .ready: connection.send(content: metadata, contentContext: .finalMessage, isComplete: true, completion: .contentProcessed { _ in finish() })
                            case .failed, .cancelled: finish()
                            default: break
                            }
                        }
                        connection.start(queue: self.queue)
                        self.queue.asyncAfter(deadline: .now() + 2) { finish() }
                        return
                    }
                    let framed = FramedConnection(connection, queue: self.queue)
                    if role == "control" {
                        guard self.control == nil else { connection.cancel(); return }
                        self.control = framed; self.attachControl(framed, usb: true)
                    } else {
                        guard self.mediaConnections < 4 else { connection.cancel(); return }; self.mediaConnections += 1
                        framed.receive = { [weak self, weak framed] data in
                            guard let self, let framed, self.generation == expected else { return }
                            do {
                                let message = try ControlMessage(data: data)
                                guard self.negotiated, message.type == "MediaBind", message.session == self.session,
                                      message.body["token"] as? String == self.token.hex, message.body["role"] as? String == role else { throw CameraError.protocolViolation("USB channel belongs to another session") }
                                if role == "video" { guard self.video == nil else { throw CameraError.protocolViolation("Video channel already bound") }; self.video = framed; let sender = USBMediaSender(connection: framed, video: true); sender.dropped = { [weak self] in self?.capture.requestIDR() }; self.usbVideo = sender }
                                else { guard self.audio == nil else { throw CameraError.protocolViolation("Audio channel already bound") }; self.audio = framed; self.usbAudio = USBMediaSender(connection: framed, video: false) }
                                framed.receive = { [weak framed] _ in framed?.close() }
                            } catch { framed.close(error) }
                        }
                        framed.failure = { [weak self, weak framed] _ in
                            guard let self, self.generation == expected else { return }
                            self.mediaConnections = max(0, self.mediaConnections - 1)
                            if self.video === framed || self.audio === framed { self.control?.close(CameraError.unavailable("USB channel disconnected")) }
                        }
                    }
                    self.queue.asyncAfter(deadline: .now() + 5) { [weak self, weak framed] in
                        guard let self, let framed, self.generation == expected else { return }
                        if role != "control" && self.video !== framed && self.audio !== framed { framed.close() }
                    }
                    framed.start()
                }
                listener.stateUpdateHandler = { [weak self] state in
                    guard let self, self.generation == expected, !self.userStopped else { return }
                    switch state {
                    case .ready:
                        self.usbReadyRoles.insert(role)
                        if self.usbReadyRoles.count == 4 {
                            self.preparingUSB = false
                            self.update?("USB ready", self.usbReadyDescription, [:])
                        }
                    case .failed(let error):
                        self.handleUSBFailure(error, attempt: attempt, port: port)
                    default: break
                    }
                }
                listeners.append(listener); listener.start(queue: queue)
            }
        } catch { handleUSBFailure(error, attempt: attempt, port: base) }
    }
    private func handleUSBFailure(_ error: Error, attempt: Int, port: UInt16) {
        if let nwError = error as? NWError, case .posix(let code) = nwError, code == .EADDRINUSE,
           attempt + 1 < Self.usbPortCandidates.count {
            generation += 1; let next = generation
            update?("Preparing USB", "Selecting another available USB port", [:])
            stopConnections { [weak self] in
                guard let self, self.generation == next, !self.userStopped else { return }
                self.startUSBListeners(attempt: attempt + 1, generation: next)
            }
        } else { fail("USB port \(port): \(error.localizedDescription)", retry: false) }
    }
    private func attachControl(_ framed: FramedConnection, usb: Bool) {
        negotiated = false; configurationPending = false; configurationPaused = false; lastVideo = 0; lastExpired = 0; lastLate = 0; lastMissing = 0
        framed.receive = { [weak self, weak framed] data in guard let self, let framed, self.control === framed else { return }; do { try self.handle(ControlMessage(data: data), usb: usb) } catch { self.fail(error.localizedDescription, retry: false) } }
        framed.failure = { [weak self, weak framed] error in guard let self, let framed, self.control === framed else { return }; self.control = nil; self.negotiated = false; self.streaming = false; self.session = ""; self.capture.stop(); self.usbVideo?.stop(); self.usbVideo = nil; self.usbAudio?.stop(); self.usbAudio = nil; self.datagrams?.stop(); self.datagrams = nil; self.video?.close(); self.video = nil; self.audio?.close(); self.audio = nil; self.heartbeat?.cancel(); self.heartbeat = nil; self.update?("Disconnected", error?.localizedDescription ?? "Connection closed", [:]); if !self.userStopped && !usb { self.retry() } }
        let expected = generation; queue.asyncAfter(deadline: .now() + 5) { [weak self, weak framed] in guard let self, let framed, self.generation == expected else { return }; if self.control === framed && !self.negotiated { framed.close(CameraError.protocolViolation("Computer did not finish connection setup")) } }
    }
    private func handle(_ message: ControlMessage, usb: Bool) throws {
        guard negotiated || message.allowedBeforeHandshake else { throw CameraError.protocolViolation("Command received before session setup") }
        guard message.epoch == 1 else { throw CameraError.protocolViolation("Unsupported transport epoch") }
        if message.type != "Hello" { guard message.session == session else { throw CameraError.protocolViolation("Stale control session") } }
        switch message.type {
        case "Hello":
            guard session.isEmpty, message.body["transport_version"] as? Int == 2, message.body["mode"] as? String == "plain",
                  let mediaText = message.body["media_token"] as? String, let mediaToken = Data(hex: mediaText), mediaToken.count == 32,
                  Data(hex: message.session)?.count == 16 else { throw CameraError.protocolViolation("Install the matching TitanCam version on your computer") }
            session = message.session; token = mediaToken; negotiated = true; reconnectAttempt = 0
            sendControl("HelloAck", ["transport_version": 2])
            heartbeat?.cancel(); let timer = DispatchSource.makeTimerSource(queue: queue)
            timer.schedule(deadline: .now(), repeating: .milliseconds(500))
            timer.setEventHandler { [weak self] in self?.sendControl("Heartbeat") }; heartbeat = timer; timer.resume()
        case "Configure":
            guard !configurationPending else {
                sendControl("ConfigureError", ["reason":"Capture configuration busy; retry after acknowledgement"], requestID: message.requestID)
                return
            }
            let data = try JSONSerialization.data(withJSONObject: message.body)
            let requested = try JSONDecoder().decode(StreamConfig.self, from: data)
            guard requested.valid, let sid = Data(hex: session) else { throw CameraError.protocolViolation("Invalid configuration") }
            let expected = generation
            configurationPending = true; configurationPaused = true
            capture.configure(requested, sessionID: sid) { [weak self] result in
                self?.queue.async {
                    guard let self, self.generation == expected, self.session == message.session else { return }
                    switch result {
                    case .success(let effective):
                        self.current = effective; self.requestedBitrate = requested.bitrate
                        self.adaptiveAt = CaptureEngine.hostTime; self.stableSince = self.adaptiveAt
                        do {
                            var body = try JSONSerialization.jsonObject(with: JSONEncoder().encode(effective)) as! [String: Any]
                            body["capabilities"] = self.capture.capabilities()
                            self.sendControl("ConfigureAck", body, requestID: message.requestID)
                            self.update?("Configured", "\(effective.width)×\(effective.height) · \(effective.fps) FPS · \(effective.codec.uppercased())", [:])
                            self.datagrams?.configure(effective)
                        } catch { self.fail(error.localizedDescription, retry: false) }
                    case .failure(let error):
                        self.configurationPending = false; self.configurationPaused = false
                        if self.streaming { self.sendControl("ConfigureError", ["reason":error.localizedDescription], requestID: message.requestID) }
                        else { self.fail(error.localizedDescription, retry: false) }
                    }
                }
            }
        case "ConfigureApplied":
            guard (message.body["config_id"] as? NSNumber)?.uint32Value == current.config_id else { return }
            configurationPending = false
            if streaming && configurationPaused { configurationPaused = false; capture.requestIDR(); capture.start() }
        case "MediaReady": if !usb && datagrams == nil { startMedia() }
        case "MediaBound": datagrams?.bound()
        case "Start":
            if usb { guard video != nil && audio != nil else { queue.asyncAfter(deadline: .now() + 0.1) { [weak self] in try? self?.handle(message, usb: usb) }; return } }
            streaming = true; sendControl("StartAck", ["host_epoch_ns": capture.epoch.description]); update?("Streaming", "\(usb ? "USB" : "Wi-Fi") · \(current.profile)", [:])
        case "StreamingReady": configurationPending = false; configurationPaused = false; capture.start()
        case "RequestIDR": capture.requestIDR()
        case "Repair": datagrams?.repair(message.body)
        case "ClockPing": let received = CaptureEngine.hostTime; sendControl("ClockPong", ["r1": message.body["r1"] as? String ?? "", "s2": received.description, "s3": CaptureEngine.hostTime.description])
        case "Feedback": if streaming { update?("Streaming", "\(usb ? "USB" : "Wi-Fi") · \(current.profile)", message.body); adapt(message.body) }
        case "Stop": capture.stop(); update?("Paused", "Receiver paused capture", [:])
        case "Error": control?.close(CameraError.unavailable("Receiver media output failed"))
        default: break
        }
    }
    private func startMedia() {
        guard let sid = Data(hex: session) else { return }
        let sender = DatagramSender(host: wifiHost, port: mediaPort, queue: queue)
        sender.configure(current); datagrams = sender
        sender.dropped = { [weak self, weak sender] in guard let self, self.datagrams === sender else { return }; self.capture.requestIDR() }
        sender.start(session: sid, token: token, ready: {}, failure: { [weak self, weak sender] error in
            guard let self, let sender, self.datagrams === sender else { return }
            self.fail(error?.localizedDescription ?? "Media connection closed", retry: true)
        })
    }
    private func send(_ unit: EncodedUnit) {
        guard streaming, !configurationPaused, unit.session.hex == session, unit.config == current.config_id else { return }
        if let datagrams { datagrams.enqueue(unit) }
        else if unit.kind == 1 { usbVideo?.enqueue(unit) } else { usbAudio?.enqueue(unit) }
    }
    private func sendControl(_ type: String, _ body: [String: Any] = [:], requestID: String = "0", completion: (() -> Void)? = nil) { guard let data = try? ControlMessage(type, session: session, body: body, requestID: requestID).data() else { return }; if control?.send(data, completion: completion) != true { fail("Control queue exceeded limit", retry: true) } }
    private func adapt(_ feedback: [String: Any]) {
        let now = CaptureEngine.hostTime; guard !configurationPending, now - adaptiveAt >= 1_000_000_000 else { return }; adaptiveAt = now
        let expired = (feedback["expired"] as? NSNumber)?.intValue ?? 0
        let frames = (feedback["video"] as? NSNumber)?.intValue ?? 0
        let missing = (feedback["missing_units"] as? NSNumber)?.intValue ?? 0
        let late = (feedback["late_video"] as? NSNumber)?.intValue ?? 0
        let rtt = (feedback["clock_rtt_ms"] as? NSNumber)?.doubleValue ?? 0
        defer { lastExpired = expired; lastVideo = frames; lastMissing = missing; lastLate = late }
        let thermal = ProcessInfo.processInfo.thermalState
        if thermal == .critical { fail("Phone is too hot. Let it cool, then reconnect.", retry: false); return }
        var next = current
        if thermal == .serious { next.fps = min(next.fps, 30); next.bitrate = min(next.bitrate, 14_000_000); if next.width > 1920 { next.width = 1920; next.height = 1080 } }
        else if expired > lastExpired || (!usb && (missing > lastMissing || late > lastLate || rtt > 40)) { stableSince = now; next.bitrate = max(2_000_000, Int(Double(next.bitrate) * 0.8)) }
        else if now - stableSince > 5_000_000_000 && frames > lastVideo { next.bitrate = min(requestedBitrate, Int(Double(next.bitrate) * 1.05)); stableSince = now }
        guard next != current, let sid = Data(hex: session) else { return }
        let formatChanged = !next.sameMediaFormat(as: current)
        if formatChanged { next.config_id += 1 }
        configurationPending = true; configurationPaused = formatChanged
        let expected = generation
        let completion: (Result<StreamConfig, Error>) -> Void = { [weak self] result in
            self?.queue.async {
                guard let self, self.generation == expected, self.session == sid.hex else { return }
                switch result {
                case .success(let effective):
                    guard let data = try? JSONEncoder().encode(effective), let body = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else { self.fail("Cannot report updated configuration", retry: false); return }
                    self.current = effective; self.datagrams?.configure(effective); self.sendControl("ConfigureAck", body)
                case .failure(let error): self.fail(error.localizedDescription, retry: false)
                }
            }
        }
        if formatChanged { capture.configure(next, sessionID: sid, completion: completion) }
        else { capture.adjustBitrate(next.bitrate, completion: completion) }
    }

    private func retry() { guard !userStopped && !wifiHost.isEmpty else { return }; reconnectAttempt += 1; let attempt = generation; let delay = min(2, pow(2, Double(min(reconnectAttempt, 4))) * 0.1); queue.asyncAfter(deadline: .now() + delay) { [weak self] in guard let self, !self.userStopped, self.generation == attempt, self.control == nil else { return }; self.session = ""; self.connectWiFi() } }
    private func fail(_ text: String, retry: Bool) {
        update?("Attention", text, [:]); capture.stop()
        if retry { control?.close(CameraError.unavailable(text)) }
        else { userStopped = true; generation += 1; stopConnections(); update?("Attention", text, [:]) }
    }
    private func stopConnections(completion: (() -> Void)? = nil) {
        heartbeat?.cancel(); heartbeat = nil
        let c = control; control = nil; c?.close()
        usbVideo?.stop(); usbVideo = nil; usbAudio?.stop(); usbAudio = nil
        video?.close(); video = nil; audio?.close(); audio = nil
        datagrams?.stop(); datagrams = nil
        capture.stop(); negotiated = false; streaming = false; session = ""; mediaConnections = 0
        usbReadyRoles.removeAll(); usbBasePort = nil
        if let completion { shutdownCompletions.append(completion) }
        // cancel() is asynchronous: do not rebind until every previous listener is cancelled.
        for connection in bootstrapConnections { connection.stateUpdateHandler = nil; connection.cancel() }; bootstrapConnections.removeAll()
        let previous = listeners; listeners.removeAll(); retiringListeners.append(contentsOf: previous)
        for listener in previous {
            listener.newConnectionHandler = { connection in connection.cancel() }
            listener.stateUpdateHandler = { [weak self, weak listener] state in
                guard let self, let listener else { return }
                if case .cancelled = state {
                    self.retiringListeners.removeAll { $0 === listener }
                    listener.stateUpdateHandler = nil; self.finishShutdownIfReady()
                }
            }
            listener.cancel()
        }
        finishShutdownIfReady()
    }
    private func finishShutdownIfReady() {
        guard retiringListeners.isEmpty else { return }
        preparingUSB = false
        let completions = shutdownCompletions; shutdownCompletions.removeAll()
        for completion in completions { completion() }
    }
    func stop(completion: (() -> Void)? = nil) {
        queue.async {
            self.userStopped = true; self.generation += 1; let expected = self.generation
            self.stopConnections { [weak self] in
                if let self, self.generation == expected { self.update?("Ready", "Choose USB or select your computer over Wi-Fi", [:]) }
                completion?()
            }
        }
    }
    static func random(_ count: Int) -> Data { var bytes = [UInt8](repeating: 0, count: count); precondition(SecRandomCopyBytes(kSecRandomDefault, count, &bytes) == errSecSuccess); return Data(bytes) }
    deinit { heartbeat?.cancel(); for listener in listeners + retiringListeners { listener.cancel() } }
}
