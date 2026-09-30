import Foundation
import Network
import CryptoKit
import UIKit
final class StreamCoordinator {
    let identity: DeviceIdentity
    let capture = CaptureEngine()
    let queue = DispatchQueue(label: "titancam.network", qos: .userInteractive)
    var update: ((String, String, [String: Any]) -> Void)?
    private var control: FramedConnection?
    private var video: FramedConnection?
    private var audio: FramedConnection?
    private var datagrams: DatagramSender?
    private var listeners: [NWListener] = []
    private var session = ""
    private var receiver = Data()
    private var nonce = Data()
    private var token = Data()
    private var usbToken = ""
    private var wifiToken = ""
    private var wifiHost = ""
    private var wifiPin = ""
    private var wifiReceiver = Data()
    private var mediaPort: UInt16 = 49161
    private var current = StreamConfig()
    private var heartbeat: DispatchSourceTimer?
    private var userStopped = true
    private var reconnectAttempt = 0
    private var generation = 0
    private var lastVideo = 0
    private var lastExpired = 0
    private var adaptiveAt: UInt64 = 0
    private var stableSince: UInt64 = 0
    private var requestedBitrate = 14_000_000
    private var usbExpires: UInt64 = 0
    var usbPairingDescription: String { "Certificate pin:\n\(identity.fingerprint)\n\nOne-time token:\n\(usbToken)" }
    init() throws {
        identity = try DeviceIdentity()
        capture.output = { [weak self] unit in self?.queue.async { self?.send(unit) } }
        capture.failure = { [weak self] error in self?.queue.async { self?.fail(error, retry: false) } }
    }
    func startWiFi(_ uri: String) {
        queue.async { do {
            self.stopConnections(); self.userStopped = false; self.generation += 1
            guard let url = URLComponents(string: uri), url.scheme == "titancam", url.host == "pair" else { throw CameraError.protocolViolation("Paste the complete titancam://pair link from the receiver") }
            let fields = Dictionary(url.queryItems?.compactMap { item in item.value.map { (item.name, $0) } } ?? [], uniquingKeysWith: { a, _ in a })
            guard let host = fields["host"], !host.isEmpty, let pin = fields["cert"], Data(hex: pin)?.count == 32, let receiver = fields["receiver"].flatMap(Data.init(hex:)), receiver.count == 32 else { throw CameraError.protocolViolation("Invalid receiver link") }
            self.wifiHost = host; self.wifiPin = pin; self.wifiReceiver = receiver; self.wifiToken = fields["token"] ?? ""; self.mediaPort = UInt16(fields["media"] ?? "49161") ?? 49161
            self.connectWiFi(port: UInt16(fields["port"] ?? "49160") ?? 49160)
        } catch { self.fail(error.localizedDescription, retry: false) } }
    }
    private func connectWiFi(port: UInt16 = 49160) {
        let connection = NWConnection(host: NWEndpoint.Host(wifiHost), port: NWEndpoint.Port(rawValue: port)!, using: Connections.clientTLS(pin: wifiPin, queue: queue)); let framed = FramedConnection(connection, queue: queue); control = framed; attachControl(framed, usb: false); framed.start(); update?("Connecting", "Authenticating receiver", [:])
    }
    func enableUSB() {
        queue.async { do { self.stopConnections(); self.userStopped = false; self.generation += 1; self.wifiHost = ""; self.usbToken = Self.random(32).hex; self.usbExpires = CaptureEngine.hostTime + 120_000_000_000
            for (port, alpn) in [(UInt16(49152), "titancam-control/1"), (49153, "titancam-media/1"), (49154, "titancam-media/1")] {
                let listener = try NWListener(using: Connections.serverTLS(identity: self.identity, alpn: alpn), on: NWEndpoint.Port(rawValue: port)!)
                listener.newConnectionHandler = { [weak self] connection in guard let self else { return }; let framed = FramedConnection(connection, queue: self.queue)
                    if port == 49152 { guard self.control == nil else { connection.cancel(); return }; self.control = framed; self.attachControl(framed, usb: true) }
                    else { framed.receive = { [weak self, weak framed] data in guard let self, let framed else { return }; do { let message = try ControlMessage(data: data); guard message.type == "MediaBind", message.session == self.session, let text = message.body["token"] as? String, text == self.token.hex, message.body["role"] as? String == (port == 49153 ? "video" : "audio") else { throw CameraError.protocolViolation("USB channel authentication failed") }; if port == 49153 { guard self.video == nil else { throw CameraError.protocolViolation("Video channel already bound") }; self.video = framed } else { guard self.audio == nil else { throw CameraError.protocolViolation("Audio channel already bound") }; self.audio = framed }; framed.receive = { _ in framed.close() } } catch { framed.close(error) } }
                        framed.failure = { [weak self] _ in self?.control?.close(CameraError.unavailable("USB channel disconnected")) }
                    }
                    // Unauthenticated channels have a strict lifetime and receive no media.
                    self.queue.asyncAfter(deadline: .now() + 5) { if port != 49152 && self.video !== framed && self.audio !== framed { framed.close() } }; framed.start()
                }
                listener.stateUpdateHandler = { [weak self] state in if case .failed(let error) = state { self?.fail(error.localizedDescription, retry: false) } }; listener.start(queue: self.queue); self.listeners.append(listener)
            }
            self.update?("USB ready", self.usbPairingDescription, [:])
        } catch { self.fail(error.localizedDescription, retry: false) } }
    }
    private func attachControl(_ framed: FramedConnection, usb: Bool) {
        framed.receive = { [weak self] data in guard let self else { return }; do { try self.handle(ControlMessage(data: data), usb: usb) } catch { self.fail(error.localizedDescription, retry: false) } }
        framed.failure = { [weak self] error in guard let self, self.control === framed else { return }; self.control = nil; self.session = ""; self.capture.stop(); self.datagrams?.stop(); self.datagrams = nil; self.video?.close(); self.video = nil; self.audio?.close(); self.audio = nil; self.heartbeat?.cancel(); self.heartbeat = nil; self.update?("Disconnected", error?.localizedDescription ?? "Connection closed", [:]); if !self.userStopped && !usb { self.retry() } }
        let expected = generation; queue.asyncAfter(deadline: .now() + 5) { [weak self, weak framed] in guard let self, let framed, self.generation == expected else { return }; if self.session.isEmpty { framed.close(CameraError.protocolViolation("Authentication timed out")) } }
    }
    private func handle(_ message: ControlMessage, usb: Bool) throws {
        guard message.epoch == 1 else { throw CameraError.protocolViolation("Unsupported transport epoch") }
        if message.type != "Hello" { guard message.session == session else { throw CameraError.protocolViolation("Stale control session") } }
        switch message.type {
        case "Hello":
            guard session.isEmpty, let publicText = message.body["receiver_public"] as? String, let publicKey = Data(hex: publicText), publicKey.count == 32, let nonceText = message.body["nonce"] as? String, let nonce = Data(hex: nonceText), nonce.count == 32, let mediaText = message.body["media_token"] as? String, let mediaToken = Data(hex: mediaText), mediaToken.count == 32, let sid = Data(hex: message.session), sid.count == 16 else { throw CameraError.protocolViolation("Invalid identity challenge") }
            if usb { let known = Keychain.load("receiver-\(publicText)") != nil; guard known || (!usbToken.isEmpty && CaptureEngine.hostTime < usbExpires && message.body["pair_token"] as? String == usbToken) else { throw CameraError.protocolViolation("Enter this phone's one-time USB token on the receiver") } }
            else { guard publicKey == wifiReceiver else { throw CameraError.protocolViolation("Receiver identity changed") } }
            session = message.session; receiver = publicKey; self.nonce = nonce; token = mediaToken
            sendControl("AuthProof", ["phone_public": identity.publicKey.hex, "signature": try identity.proof(receiver: publicKey, nonce: nonce, session: sid), "pair_token": usb ? usbToken : wifiToken])
        case "AuthOk":
            guard let signature = message.body["signature"] as? String, let sid = Data(hex: session) else { throw CameraError.protocolViolation("Invalid receiver proof") }; try identity.verify(signature: signature, receiver: receiver, nonce: nonce, session: sid); try Keychain.save("receiver-\(receiver.hex)", receiver); reconnectAttempt = 0
            heartbeat?.cancel(); let timer = DispatchSource.makeTimerSource(queue: queue); timer.schedule(deadline: .now(), repeating: .milliseconds(500)); timer.setEventHandler { [weak self] in self?.sendControl("Heartbeat") }; heartbeat = timer; timer.resume()
        case "Configure":
            let data = try JSONSerialization.data(withJSONObject: message.body); let requested = try JSONDecoder().decode(StreamConfig.self, from: data); guard let sid = Data(hex: session) else { throw CameraError.protocolViolation("Invalid session") }
            capture.configure(requested, sessionID: sid) { [weak self] result in self?.queue.async { guard let self else { return }; switch result { case .success(let effective): self.current = effective; self.requestedBitrate = requested.bitrate; self.adaptiveAt = CaptureEngine.hostTime; self.stableSince = self.adaptiveAt; do { let body = try JSONSerialization.jsonObject(with: JSONEncoder().encode(effective)) as! [String: Any]; self.sendControl("ConfigureAck", body); self.update?("Configured", "\(effective.width)×\(effective.height) · \(effective.fps) FPS target · \(effective.codec.uppercased())", [:]); if !usb && self.datagrams == nil { self.startMedia() } else { self.datagrams?.configure(effective) } } catch { self.fail(error.localizedDescription, retry: false) }; case .failure(let error): self.fail(error.localizedDescription, retry: false) } } }
        case "MediaBound": datagrams?.bound()
        case "Start":
            if usb { guard video != nil && audio != nil else { queue.asyncAfter(deadline: .now() + 0.1) { [weak self] in try? self?.handle(message, usb: usb) }; return } }
            sendControl("StartAck", ["host_epoch_ns": capture.epoch.description], completion: { [weak self] in self?.capture.start() }); update?("Streaming", "Encrypted \(usb ? "USB" : "Wi-Fi") · \(current.profile)", [:])
        case "RequestIDR": capture.requestIDR()
        case "ClockPing": let received = CaptureEngine.hostTime; sendControl("ClockPong", ["r1": message.body["r1"] as? String ?? "", "s2": received.description, "s3": CaptureEngine.hostTime.description])
        case "Feedback": update?("Streaming", "Encrypted \(usb ? "USB" : "Wi-Fi") · \(current.profile)", message.body); adapt(message.body)
        case "Stop": capture.stop(); update?("Paused", "Receiver paused capture", [:])
        case "Error": control?.close(CameraError.unavailable("Receiver media output failed"))
        default: break
        }
    }
    private func startMedia() { guard let sid = Data(hex: session) else { return }; let sender = DatagramSender(host: wifiHost, port: mediaPort, pin: wifiPin, queue: queue); sender.configure(current); sender.dropped = { [weak self] in self?.capture.requestIDR() }; datagrams = sender; sender.start(session: sid, token: token, ready: {}, failure: { [weak self] error in self?.fail(error?.localizedDescription ?? "Media connection closed", retry: true) }) }
    private func send(_ unit: EncodedUnit) { guard unit.session.hex == session else { return }; if let datagrams { datagrams.enqueue(unit) } else { let channel = unit.kind == 1 ? video : audio; if channel?.send(unit.packet(), media: true) != true { if unit.kind == 1 { capture.requestIDR(); if let channel { channel.close(CameraError.unavailable("USB media backlog")) } } } } }
    private func sendControl(_ type: String, _ body: [String: Any] = [:], completion: (() -> Void)? = nil) { guard let data = try? ControlMessage(type, session: session, body: body).data() else { return }; if control?.send(data, completion: completion) != true { fail("Control queue exceeded limit", retry: true) } }
    private func adapt(_ feedback: [String: Any]) {
        let now = CaptureEngine.hostTime; guard now - adaptiveAt >= 1_000_000_000 else { return }; adaptiveAt = now
        let expired = (feedback["expired"] as? NSNumber)?.intValue ?? 0; let frames = (feedback["video"] as? NSNumber)?.intValue ?? 0; defer { lastExpired = expired; lastVideo = frames }
        let thermal = ProcessInfo.processInfo.thermalState
        if thermal == .critical { fail("Phone is too hot. Let it cool, then reconnect.", retry: false); return }
        var next = current
        if thermal == .serious { next.fps = min(next.fps, 30); next.bitrate = min(next.bitrate, 14_000_000); if next.width > 1920 { next.width = 1920; next.height = 1080 } }
        else if expired > lastExpired { stableSince = now; next.bitrate = max(2_000_000, Int(Double(next.bitrate) * 0.8)) }
        else if now - stableSince > 5_000_000_000 && frames > lastVideo { next.bitrate = min(requestedBitrate, Int(Double(next.bitrate) * 1.05)); stableSince = now }
        guard next != current, let sid = Data(hex: session) else { return }; next.config_id += 1
        capture.configure(next, sessionID: sid) { [weak self] result in self?.queue.async { guard let self else { return }; if case .success(let effective) = result, let data = try? JSONEncoder().encode(effective), let body = try? JSONSerialization.jsonObject(with: data) as? [String: Any] { self.current = effective; self.datagrams?.configure(effective); self.sendControl("ConfigureAck", body); self.capture.requestIDR() } } }
    }
    private func retry() { guard !userStopped && !wifiHost.isEmpty else { return }; reconnectAttempt += 1; let attempt = generation; let delay = min(2, pow(2, Double(min(reconnectAttempt, 4))) * 0.1); queue.asyncAfter(deadline: .now() + delay) { [weak self] in guard let self, !self.userStopped, self.generation == attempt, self.control == nil else { return }; self.session = ""; self.connectWiFi() } }
    private func fail(_ text: String, retry: Bool) { update?("Attention", text, [:]); capture.stop(); if retry { control?.close(CameraError.unavailable(text)) } else { userStopped = true; stopConnections(); update?("Attention", text, [:]) } }
    private func stopConnections() { heartbeat?.cancel(); heartbeat = nil; let c = control; control = nil; c?.close(); video?.close(); video = nil; audio?.close(); audio = nil; datagrams?.stop(); datagrams = nil; for listener in listeners { listener.cancel() }; listeners.removeAll(); capture.stop(); session = "" }
    func stop() { queue.async { self.userStopped = true; self.generation += 1; self.stopConnections(); self.update?("Ready", "Choose USB or pair over Wi-Fi", [:]) } }
    static func random(_ count: Int) -> Data { var bytes = [UInt8](repeating: 0, count: count); precondition(SecRandomCopyBytes(kSecRandomDefault, count, &bytes) == errSecSuccess); return Data(bytes) }
    deinit { heartbeat?.cancel(); for listener in listeners { listener.cancel() } }
}
