import XCTest
import CryptoKit
@testable import TitanCam
final class ProtocolTests: XCTestCase {
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
}
