import SwiftUI
import UIKit
@MainActor
final class AppModel: ObservableObject {
    @Published var status = "Ready"
    @Published var detail = ""
    var coordinator: StreamCoordinator?
    private var resumeConnection: (Bool, String)?
    private var backgroundPaused = false
    private var connectionTask: Task<Void, Never>?
    var authorize: () async -> Bool = { await CaptureEngine.authorize() }
    @Published var dimmed = false
    private var savedBrightness: CGFloat?
    func toggleDim() {
        dimmed.toggle()
        if dimmed { savedBrightness = UIScreen.main.brightness; UIScreen.main.brightness = 0.03 }
        else { restoreBrightness() }
    }
    func restoreBrightness() {
        if let savedBrightness { UIScreen.main.brightness = savedBrightness }
        savedBrightness = nil; dimmed = false
    }
    func stop() { connectionTask?.cancel(); connectionTask = nil; resumeConnection = nil; backgroundPaused = false; coordinator?.stop(); restoreBrightness(); UIApplication.shared.isIdleTimerDisabled = false }
    func background() { connectionTask?.cancel(); connectionTask = nil; backgroundPaused = resumeConnection != nil; coordinator?.stop(); restoreBrightness() }
    func foreground() {
        if backgroundPaused, let (usb, uri) = resumeConnection { backgroundPaused = false; connect(usb: usb, uri: uri) }
    }
    init() {
        do {
            let stream = try StreamCoordinator(); coordinator = stream
            stream.update = { [weak self] status, detail, _ in
                DispatchQueue.main.async {
                    guard let self else { return }
                    self.status = status; self.detail = detail
                    UIApplication.shared.isIdleTimerDisabled = status == "Streaming"
                    if ["Attention", "Disconnected", "Paused", "Ready"].contains(status) { self.restoreBrightness() }
                }
            }
        } catch { status = "Setup failed"; detail = error.localizedDescription }
    }
    func connect(usb: Bool, uri: String = "") {
        resumeConnection = (usb, uri)
        connectionTask?.cancel()
        connectionTask = Task {
            let permitted = await authorize()
            guard !Task.isCancelled else { return }
            guard permitted else { status = "Permission needed"; detail = "Allow Camera and Microphone in Settings."; return }
            if usb { coordinator?.enableUSB() } else { coordinator?.startWiFi(uri) }
        }
    }
}
struct ContentView: View {
    @StateObject private var model = AppModel()
    @StateObject private var discovery = ReceiverDiscovery()
    @Environment(\.scenePhase) private var phase
    var body: some View {
        VStack(spacing: 24) {
            Spacer()
            Image(systemName: "video.fill").font(.system(size: 46)).foregroundStyle(.mint)
            Text("TitanCam").font(.largeTitle.bold())
            Text("Version \(Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "?") · Build \(Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion") as? String ?? "?")").font(.caption).foregroundStyle(.secondary)
            Text(model.status).font(.title3).foregroundStyle(model.status == "Streaming" ? .mint : .secondary)
            if !model.detail.isEmpty { Text(model.detail).font(.footnote).foregroundStyle(.secondary).multilineTextAlignment(.center) }
            Button { model.connect(usb: true) } label: { Label("Connect with USB", systemImage: "cable.connector").frame(maxWidth: .infinity).padding(10) }.buttonStyle(.borderedProminent).tint(.mint)
            HStack { Text("Wi-Fi computers").font(.headline); Spacer(); Button { discovery.refresh() } label: { Image(systemName: "arrow.clockwise") }.accessibilityLabel("Refresh computers") }
            if discovery.receivers.isEmpty { Text(discovery.message).font(.footnote).foregroundStyle(.secondary).multilineTextAlignment(.center) }
            ScrollView {
                VStack(spacing: 10) {
                    ForEach(discovery.receivers) { receiver in
                        Button { model.connect(usb: false, uri: receiver.connectionURI) } label: { Label(receiver.name, systemImage: "desktopcomputer").frame(maxWidth: .infinity, alignment: .leading).padding(10) }.buttonStyle(.bordered)
                    }
                }
            }.frame(maxHeight: 180)
            Button(model.dimmed ? "Restore brightness" : "Dim screen — keep app open") { model.toggleDim() }.buttonStyle(.bordered)
            Button("Stop", role: .destructive) { model.stop() }.buttonStyle(.bordered)
            Spacer()
        }.padding(28).preferredColorScheme(.dark).background(model.dimmed ? Color.black : Color.clear)
        .onAppear { discovery.start() }
        .onChange(of: phase) { phase in
            // System permission dialogs temporarily make the scene inactive.
            if phase == .background { model.background(); discovery.stop() }
            if phase == .active { discovery.start(); model.foreground() }
            UIApplication.shared.isIdleTimerDisabled = phase == .active && model.status == "Streaming"
        }
    }
}
