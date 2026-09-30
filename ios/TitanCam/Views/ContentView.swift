import SwiftUI
import UIKit
@MainActor
final class AppModel: ObservableObject {
    @Published var status = "Ready"
    @Published var detail = ""
    var coordinator: StreamCoordinator?
    init() {
        do {
            let stream = try StreamCoordinator(); coordinator = stream
            stream.update = { [weak self] status, detail, _ in
                DispatchQueue.main.async { self?.status = status; self?.detail = detail }
            }
        } catch { status = "Setup failed"; detail = error.localizedDescription }
    }
    func connect(usb: Bool, uri: String = "") {
        Task {
            guard await CaptureEngine.authorize() else { status = "Permission needed"; detail = "Allow Camera and Microphone in Settings."; return }
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
            Button("Stop", role: .destructive) { model.coordinator?.stop() }.buttonStyle(.bordered)
            Spacer()
        }.padding(28).preferredColorScheme(.dark)
        .onAppear { UIApplication.shared.isIdleTimerDisabled = true; discovery.start() }
        .onChange(of: phase) { phase in
            // System permission dialogs temporarily make the scene inactive.
            if phase == .background { model.coordinator?.stop(); discovery.stop() }
            if phase == .active { discovery.start() }
            UIApplication.shared.isIdleTimerDisabled = phase == .active
        }
    }
}
