import XCTest
import Network
@testable import TitanCam
final class ProtocolTests: XCTestCase {
    func testCommandsRequireSessionSetup() {
        for command in ["Configure", "Start", "MediaReady", "Feedback"] { XCTAssertFalse(ControlMessage(command, session: String(repeating:"0",count:32)).allowedBeforeHandshake) }
        XCTAssertTrue(ControlMessage("Hello",session:String(repeating:"0",count:32)).allowedBeforeHandshake)
    }
    func testSharedRustGoldenBytes() throws {
        let url = try XCTUnwrap(Bundle(for: ProtocolTests.self).url(forResource: "media-header-v1", withExtension: "bin"))
        let unit = EncodedUnit(kind: 1, independent: true, session: Data(repeating: 7, count: 16), config: 1, sequence: 1, pts: 123, duration: 16_666_667, bytes: Data("abcdef".utf8))
        XCTAssertEqual(Data(unit.packet(index: 0, count: 2, offset: 0, length: 3).prefix(64)), try Data(contentsOf: url))
    }
    func testHeaderGoldenLayout() {
        let unit = EncodedUnit(kind: 1, independent: true, session: Data(repeating: 0x11, count: 16), config: 7, sequence: 9, pts: 123_456, duration: 16_666_666, bytes: Data([1,2,3]))
        let packet = unit.packet(); XCTAssertEqual(packet.count, 67); XCTAssertEqual(String(data: packet.prefix(4), encoding: .utf8), "TCAM"); XCTAssertEqual(packet.integer(at: 24, UInt32.self), 1); XCTAssertEqual(packet.integer(at: 28, UInt32.self), 7); XCTAssertEqual(packet.integer(at: 32, UInt64.self), 9); XCTAssertEqual(packet.integer(at: 40, UInt64.self), 123_456); XCTAssertEqual(packet.integer(at: 52, UInt32.self), 3); XCTAssertEqual(packet.integer(at: 58, UInt16.self), 1)
    }
    func testFragmentsKeepOriginalLength() { let unit = EncodedUnit(kind: 1, independent: true, session: Data(repeating: 0, count: 16), config: 1, sequence: 0, pts: 0, duration: 1, bytes: Data(repeating: 7, count: 2000)); let packet = unit.packet(index: 1, count: 2, offset: 1036, length: 964); XCTAssertEqual(packet.integer(at: 52, UInt32.self), 2000); XCTAssertEqual(packet.integer(at: 60, UInt32.self), 1036); XCTAssertEqual(packet.count, 1028) }
    func testInvalidControlAndProfiles() { XCTAssertThrowsError(try ControlMessage(data: Data("{}".utf8))); XCTAssertThrowsError(try ControlMessage(data: Data(repeating: 0, count: 65_537))); var config = StreamConfig(); config.playout_ms = -1; XCTAssertFalse(config.valid); config.playout_ms = 10; XCTAssertTrue(config.valid); XCTAssertNil(Data(hex: "zz")) }
    func testDiscoveryRequiresMatchingPlainTransport() throws {
        let fields = ["v":"2", "mode":"plain", "host":"192.168.8.4", "media":"49161"]
        let receiver = try XCTUnwrap(DiscoveredReceiver(name:"TitanCam — Linux", fields:fields))
        XCTAssertFalse(receiver.connectionURI.contains("token=")); XCTAssertFalse(receiver.connectionURI.contains("cert="))
        XCTAssertEqual(receiver.host,"192.168.8.4")
        var invalid = fields; invalid["v"] = "1"; XCTAssertNil(DiscoveredReceiver(name:"Old",fields:invalid))
        invalid = fields; invalid["mode"] = "encrypted"; XCTAssertNil(DiscoveredReceiver(name:"Old",fields:invalid))
    }
    func testUSBPlainHandshakeWithoutAuthentication() throws {
        let coordinator = try StreamCoordinator(); defer { stopAndWait(coordinator) }
        let ready = expectation(description:"USB ready")
        coordinator.update = { status, _, _ in if status == "USB ready" {ready.fulfill()} }
        coordinator.enableUSB(); wait(for:[ready],timeout:10)
        let base = try XCTUnwrap(coordinator.queue.sync { coordinator.usbBasePort })
        let metadata = expectation(description:"Bounded plain USB metadata")
        let bootstrap = NWConnection(host:"127.0.0.1",port:NWEndpoint.Port(rawValue:base-1)!,using:Connections.tcp())
        defer { bootstrap.cancel() }
        bootstrap.stateUpdateHandler = { state in
            if case .ready = state {
                bootstrap.receive(minimumIncompleteLength:1,maximumLength:256) { data,_,_,error in
                    XCTAssertNil(error)
                    if let data, let value = try? JSONSerialization.jsonObject(with:data) as? [String:Int] {
                        XCTAssertEqual(value["version"],2); XCTAssertEqual(value["base"],Int(base)); metadata.fulfill()
                    } else { XCTFail("Invalid USB metadata") }
                }
            }
        }
        bootstrap.start(queue:DispatchQueue(label:"test.bootstrap"))
        wait(for:[metadata],timeout:10)
        let queue = DispatchQueue(label:"test.plain-handshake")
        let framed = FramedConnection(NWConnection(host:"127.0.0.1",port:NWEndpoint.Port(rawValue:base)!,using:Connections.tcp()),queue:queue)
        defer { queue.sync { framed.close() } }
        let ack = expectation(description:"HelloAck over plain TCP")
        framed.receive = { bytes in
            if let message = try? ControlMessage(data:bytes), message.type == "HelloAck" { XCTAssertEqual(message.body["transport_version"] as? Int,2); ack.fulfill() }
        }
        let hello = ControlMessage("Hello",session:String(repeating:"1",count:32),body:["transport_version":2,"mode":"plain","media_token":String(repeating:"a",count:64)])
        let bytes = try hello.data()
        queue.async { framed.start(); XCTAssertTrue(framed.send(bytes)) }
        wait(for:[ack],timeout:10)
    }
    private func stopAndWait(_ coordinator: StreamCoordinator) {
        let cancelled = expectation(description: "Coordinator listeners fully cancelled")
        coordinator.stop { cancelled.fulfill() }; wait(for: [cancelled], timeout: 10)
    }
    private func cancelAndWait(_ listener: NWListener) {
        let cancelled = expectation(description: "Test listener fully cancelled")
        listener.stateUpdateHandler = { state in if case .cancelled = state { cancelled.fulfill() } }
        listener.cancel(); wait(for: [cancelled], timeout: 10)
    }
    func testUSBRepeatedEnableAndImmediateRestart() throws {
        let coordinator = try StreamCoordinator()
        defer { stopAndWait(coordinator) }
        func awaitReady(_ operation: () -> Void) {
            let ready = expectation(description: "All four USB listeners ready")
            coordinator.update = { status, _, _ in
                if status == "USB ready" { ready.fulfill() }
                if status == "Attention" { XCTFail("USB listener startup failed") }
            }
            operation(); wait(for: [ready], timeout: 10)
        }
        awaitReady { coordinator.enableUSB(); coordinator.enableUSB() }
        awaitReady { coordinator.enableUSB() } // Keep the existing listener set without rebinding.
        coordinator.stop()
        awaitReady { coordinator.enableUSB() } // Queued restart must await asynchronous cancellation.
    }
    func testUSBPortConflictSelectsAlternative() throws {
        let queue = DispatchQueue(label: "test.occupied-port")
        let blocker = try NWListener(using: .tcp, on: NWEndpoint.Port(rawValue: StreamCoordinator.usbPortCandidates[0])!)
        let occupied = expectation(description: "Port occupied")
        blocker.newConnectionHandler = { $0.cancel() }
        blocker.stateUpdateHandler = { state in if case .ready = state { occupied.fulfill() } }
        blocker.start(queue: queue); wait(for: [occupied], timeout: 10)
        defer { cancelAndWait(blocker) }
        let coordinator = try StreamCoordinator(); defer { stopAndWait(coordinator) }
        let ready = expectation(description: "Fallback USB ports ready")
        coordinator.update = { status, _, _ in
            if status == "USB ready" {
                XCTAssertEqual(coordinator.usbBasePort, StreamCoordinator.usbPortCandidates[1]); ready.fulfill()
            }
            if status == "Attention" { XCTFail("Port conflict was not recovered") }
        }
        coordinator.enableUSB(); wait(for: [ready], timeout: 10)
    }
    func testSwitchFromUSBToWiFiReleasesListeners() throws {
        let coordinator = try StreamCoordinator(); defer { stopAndWait(coordinator) }
        let ready = expectation(description: "USB ready")
        coordinator.update = { status, _, _ in if status == "USB ready" { ready.fulfill() } }
        coordinator.enableUSB(); wait(for: [ready], timeout: 10)
        let switched = expectation(description: "Wi-Fi starts after listener cancellation")
        coordinator.update = { status, detail, _ in
            if status == "Connecting" && detail == "Connecting to your computer" { switched.fulfill() }
        }
        coordinator.startWiFi("titancam://local?host=127.0.0.1&port=1")
        wait(for: [switched], timeout: 10)
        let rebound = expectation(description: "Former USB port can be bound")
        let probe = try NWListener(using: .tcp, on: NWEndpoint.Port(rawValue: StreamCoordinator.usbPortCandidates[0])!)
        probe.newConnectionHandler = { $0.cancel() }
        probe.stateUpdateHandler = { state in
            if case .ready = state { rebound.fulfill() }
            if case .failed = state { XCTFail("USB listener was not released before Wi-Fi") }
        }
        probe.start(queue: DispatchQueue(label: "test.rebind")); defer { cancelAndWait(probe) }
        wait(for: [rebound], timeout: 10)
    }

}
