import XCTest
import CryptoKit
import Network
@testable import TitanCam
final class ProtocolTests: XCTestCase {
    func testKeychainIdentityPersists() throws {
        let first = try DeviceIdentity(); let second = try DeviceIdentity()
        XCTAssertEqual(first.publicKey, second.publicKey)
        XCTAssertEqual(first.fingerprint, second.fingerprint)
        XCTAssertEqual(first.certificate, second.certificate)
    }
    func testCommandsRequireAuthentication() {
        for command in ["Configure", "Start", "StreamingReady", "ConfigureApplied", "MediaReady", "Feedback", "ClockPing"] { XCTAssertFalse(ControlMessage(command, session: String(repeating: "0", count: 32)).allowedBeforeAuthentication) }
        XCTAssertTrue(ControlMessage("AuthOk", session: String(repeating: "0", count: 32)).allowedBeforeAuthentication)
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
    func testRoleAndNonceAreBound() throws { let privateKey = Curve25519.Signing.PrivateKey(); let phone = privateKey.publicKey.rawRepresentation; let receiver = Data(repeating: 2, count: 32); let nonce = Data(repeating: 3, count: 32); let session = Data(repeating: 4, count: 16); let a = DeviceIdentity.transcript(role: 1, phone: phone, receiver: receiver, nonce: nonce, session: session); let signature = try privateKey.signature(for: a); XCTAssertTrue(privateKey.publicKey.isValidSignature(signature, for: a)); XCTAssertFalse(privateKey.publicKey.isValidSignature(signature, for: DeviceIdentity.transcript(role: 2, phone: phone, receiver: receiver, nonce: nonce, session: session))); XCTAssertFalse(privateKey.publicKey.isValidSignature(signature, for: DeviceIdentity.transcript(role: 1, phone: phone, receiver: receiver, nonce: Data(repeating: 0, count: 32), session: session))) }
    func testInvalidControlAndProfiles() { XCTAssertThrowsError(try ControlMessage(data: Data("{}".utf8))); XCTAssertThrowsError(try ControlMessage(data: Data(repeating: 0, count: 65_537))); var config = StreamConfig(); config.playout_ms = -1; XCTAssertFalse(config.valid); config.playout_ms = 10; XCTAssertTrue(config.valid); XCTAssertNil(Data(hex: "zz")) }
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
            let ready = expectation(description: "All three USB listeners ready")
            coordinator.update = { status, _, _ in
                if status == "USB ready" { ready.fulfill() }
                if status == "Attention" { XCTFail("USB listener startup failed") }
            }
            operation(); wait(for: [ready], timeout: 10)
        }
        awaitReady { coordinator.enableUSB(); coordinator.enableUSB() }
        awaitReady { coordinator.enableUSB() } // Renew token without rebinding an existing set.
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
        coordinator.update = { status, detail, _ in
            if status == "USB ready" {
                XCTAssertTrue(detail.contains("USB base port:\n\(StreamCoordinator.usbPortCandidates[1])")); ready.fulfill()
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
            if status == "Connecting" && detail == "Authenticating receiver" { switched.fulfill() }
        }
        let zeros = String(repeating: "0", count: 64)
        coordinator.startWiFi("titancam://pair?host=127.0.0.1&port=1&cert=\(zeros)&receiver=\(zeros)")
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
