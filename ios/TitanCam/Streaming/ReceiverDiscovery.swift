import Foundation
import Network

struct DiscoveredReceiver: Identifiable, Equatable {
    let id: String
    let name: String
    let host: String
    let mediaPort: UInt16
    init?(name: String, fields: [String: String]) {
        guard fields["v"] == "2", fields["mode"] == "plain",
              let host = fields["host"], !host.isEmpty, host.utf8.count <= 253,
              !host.contains(where: { $0.isWhitespace }),
              let media = UInt16(fields["media"] ?? "49161"), media > 0 else { return nil }
        self.id = name + "@" + host; self.name = name; self.host = host
        self.mediaPort = media
    }
    var connectionURI: String {
        var url = URLComponents(); url.scheme = "titancam"; url.host = "local"
        url.queryItems = [URLQueryItem(name: "host", value: host), URLQueryItem(name: "port", value: "49160"), URLQueryItem(name: "media", value: String(mediaPort))]
        return url.string!
    }
}
@MainActor
final class ReceiverDiscovery: ObservableObject {
    @Published var receivers: [DiscoveredReceiver] = []
    @Published var message = "Searching for computers on your local network…"
    private var browser: NWBrowser?
    private let queue = DispatchQueue(label: "titancam.discovery")
    func start() {
        guard browser == nil else { return }
        let parameters = NWParameters.tcp; parameters.includePeerToPeer = false
        let browser = NWBrowser(for: .bonjourWithTXTRecord(type: "_titancam._tcp", domain: nil), using: parameters)
        self.browser = browser
        browser.browseResultsChangedHandler = { [weak self] results, _ in
            let found = results.compactMap { result -> DiscoveredReceiver? in
                guard case .service(let name, _, _, _) = result.endpoint, case .bonjour(let txt) = result.metadata else { return nil }
                return DiscoveredReceiver(name: name, fields: txt.dictionary)
            }.sorted { $0.name.localizedCaseInsensitiveCompare($1.name) == .orderedAscending }
            DispatchQueue.main.async { [weak self] in self?.receivers = found; self?.message = found.isEmpty ? "No computer found yet. Keep TitanCam open on Linux and use the same Wi-Fi." : "Choose your computer to connect." }
        }
        browser.stateUpdateHandler = { [weak self] state in
            if case .waiting = state { DispatchQueue.main.async { self?.message = "Allow Local Network access in Settings and check your Wi-Fi connection." } }
            if case .failed = state { DispatchQueue.main.async { self?.message = "Discovery stopped. Tap Refresh to search again."; self?.stop() } }
        }
        browser.start(queue: queue)
    }
    func stop() { browser?.cancel(); browser = nil; receivers = [] }
    func refresh() { stop(); start() }
    deinit { browser?.cancel() }
}
