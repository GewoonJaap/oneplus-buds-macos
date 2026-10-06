import SwiftUI
import AppKit
import UserNotifications
import ServiceManagement

// MARK: - State from the Rust daemon

struct CellState: Codable, Equatable { var percent: Int; var charging: Bool }
struct BatteryState: Codable, Equatable { var left: CellState?; var right: CellState?; var `case`: CellState? }
struct GestureSet: Codable, Equatable {
    var press: String?
    var double: String?
    var triple: String?
    var swipe: String?

    func action(_ g: String) -> String? {
        switch g {
        case "press": return press
        case "double": return double
        case "triple": return triple
        default: return swipe
        }
    }
}
struct Gestures: Codable, Equatable { var left: GestureSet; var right: GestureSet }
struct BudsState: Codable, Equatable {
    var game: Bool?
    var gestures: Gestures?
    var hold: Int?
    var connected = false
    var mode: String?
    var battery: BatteryState?
    var eq: String?
    var spatial: String?
}

final class NotificationDelegate: NSObject, UNUserNotificationCenterDelegate {
    static let shared = NotificationDelegate()
    func userNotificationCenter(_ center: UNUserNotificationCenter, willPresent notification: UNNotification, withCompletionHandler completionHandler: @escaping (UNNotificationPresentationOptions) -> Void) {
        completionHandler([.banner, .sound])
    }
}

enum Notifier {
    private static func log(_ m: String) {
        let url = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Application Support/Buds/notif.log")
        let line = "\(Date()) \(m)\n"
        if let h = try? FileHandle(forWritingTo: url) { h.seekToEndOfFile(); h.write(Data(line.utf8)); try? h.close() }
        else { try? line.write(to: url, atomically: true, encoding: .utf8) }
    }

    static func requestAuth(then: (() -> Void)? = nil) {
        let c = UNUserNotificationCenter.current()
        c.delegate = NotificationDelegate.shared
        c.getNotificationSettings { st in
            log("settings status=\(st.authorizationStatus.rawValue)")
            if st.authorizationStatus == .denied {
                DispatchQueue.main.async {
                    if let u = URL(string: "x-apple.systempreferences:com.apple.Notifications-Settings.extension") { NSWorkspace.shared.open(u) }
                }
                return
            }
            c.requestAuthorization(options: [.alert, .sound]) { granted, err in
                log("auth granted=\(granted) err=\(String(describing: err))")
                if granted { then?() }
            }
        }
    }

    static func post(_ title: String, _ body: String) {
        UNUserNotificationCenter.current().delegate = NotificationDelegate.shared
        let c = UNMutableNotificationContent()
        c.title = title
        c.body = body
        c.sound = .default
        UNUserNotificationCenter.current().add(UNNotificationRequest(identifier: UUID().uuidString, content: c, trigger: nil)) { err in
            log("post err=\(String(describing: err))")
        }
    }
}

final class Daemon: ObservableObject {
    @Published var state = BudsState() { didSet { evaluate(old: oldValue, new: state) } }
    private var lowNotified = Set<String>()
    private var proc: Process?
    private var stdin: FileHandle?
    private var buffer = Data()

    init() { start() }

    func start() {
        guard let exe = Bundle.main.executableURL?.deletingLastPathComponent().appendingPathComponent("buds-daemon") else { return }
        let p = Process()
        p.executableURL = exe
        let out = Pipe(), inp = Pipe()
        p.standardOutput = out
        p.standardInput = inp
        out.fileHandleForReading.readabilityHandler = { [weak self] h in
            let d = h.availableData
            guard !d.isEmpty else { return }
            DispatchQueue.main.async { self?.ingest(d) }
        }
        try? p.run()
        proc = p
        stdin = inp.fileHandleForWriting
    }

    private func ingest(_ d: Data) {
        buffer.append(d)
        while let nl = buffer.firstIndex(of: 10) {
            let line = buffer.subdata(in: buffer.startIndex..<nl)
            buffer.removeSubrange(buffer.startIndex...nl)
            if let s = try? JSONDecoder().decode(BudsState.self, from: line) { state = s }
        }
    }

    private func evaluate(old: BudsState, new: BudsState) {
        let d = UserDefaults.standard
        if d.bool(forKey: "notifyConnection"), old.connected != new.connected {
            Notifier.post(new.connected ? "Buds connected" : "Buds disconnected", "OnePlus Buds Pro 3")
        }
        guard d.bool(forKey: "notifyLow") else { return }
        let threshold = d.object(forKey: "lowThreshold") as? Int ?? 20
        let cells: [(String, CellState?)] = [("Left earbud", new.battery?.left), ("Right earbud", new.battery?.right), ("Charging case", new.battery?.case)]
        for (name, cell) in cells {
            guard let c = cell else { continue }
            if !c.charging && c.percent <= threshold {
                if lowNotified.insert(name).inserted {
                    Notifier.post("\(name) battery low", "\(c.percent)% remaining")
                }
            } else if c.charging || c.percent > threshold + 5 {
                lowNotified.remove(name)
            }
        }
    }

    func send(_ cmd: String) {
        try? stdin?.write(contentsOf: Data((cmd + "\n").utf8))
    }

    func setMode(_ m: String) { state.mode = m; send("mode \(m)") }
    func setEq(_ e: String) { state.eq = e; send("eq \(e)") }
    func setSpatial(_ s: String) { state.spatial = s; send("spatial \(s)") }
    func setGame(_ on: Bool) { state.game = on; send("game \(on ? "on" : "off")") }
    func setHold(_ mask: Int) { state.hold = mask; send("hold \(mask)") }
    func setGesture(side: String, gesture: String, action: String) {
        if var g = state.gestures {
            func apply(_ set: inout GestureSet) {
                switch gesture {
                case "press": set.press = action
                case "double": set.double = action
                case "triple": set.triple = action
                default: set.swipe = action
                }
            }
            if side == "left" { apply(&g.left) } else { apply(&g.right) }
            state.gestures = g
        }
        send("gesture \(side) \(gesture) \(action)")
    }
}

// MARK: - Views

struct BatteryRing: View {
    let title: String
    let symbol: String
    let cell: CellState?

    var body: some View {
        VStack(spacing: 6) {
            ZStack {
                Circle().stroke(.quaternary, lineWidth: 5)
                if let c = cell {
                    Circle()
                        .trim(from: 0, to: CGFloat(c.percent) / 100)
                        .stroke(c.percent <= 20 && !c.charging ? Color.red : Color.green,
                                style: StrokeStyle(lineWidth: 5, lineCap: .round))
                        .rotationEffect(.degrees(-90))
                }
                Image(systemName: symbol).font(.system(size: 17)).foregroundStyle(.primary)
                if cell?.charging == true {
                    Image(systemName: "bolt.fill").font(.system(size: 9)).foregroundStyle(.green)
                        .offset(y: -17).opacity(0)
                }
            }
            .frame(width: 52, height: 52)
            HStack(spacing: 2) {
                if cell?.charging == true { Image(systemName: "bolt.fill").font(.system(size: 9)).foregroundStyle(.green) }
                Text(cell.map { "\($0.percent)%" } ?? "–").font(.system(size: 12, weight: .medium)).monospacedDigit()
            }
            Text(title).font(.system(size: 11)).foregroundStyle(.secondary)
        }
        .frame(maxWidth: .infinity)
    }
}

struct ModeOption: Identifiable {
    let id: String   // "off", "transparency", "adaptive", "nc"
    let label: String
    let symbol: String
}

let modeOptions = [
    ModeOption(id: "off", label: "Off", symbol: "person.crop.circle"),
    ModeOption(id: "transparency", label: "Trans-\nparency", symbol: "person.wave.2"),
    ModeOption(id: "adaptive", label: "Adaptive", symbol: "sparkles"),
    ModeOption(id: "nc", label: "Noise\nCancellation", symbol: "person.crop.circle.fill"),
]

func group(of mode: String?) -> String? {
    switch mode {
    case "high", "medium", "low": return "nc"
    default: return mode
    }
}

struct Thumb: View {
    var body: some View {
        if #available(macOS 26, *) {
            Color.clear.glassEffect(.regular.interactive(), in: .rect(cornerRadius: 10))
        } else {
            RoundedRectangle(cornerRadius: 10, style: .continuous)
                .fill(Color(nsColor: .controlBackgroundColor))
                .shadow(color: .black.opacity(0.15), radius: 2, y: 1)
        }
    }
}

struct SegmentedIcons: View {
    let options: [ModeOption]
    let selected: String?
    let enabled: Bool
    let onSelect: (String) -> Void
    @State private var dragIndex: Int?
    @State private var pressed = false

    private let pad: CGFloat = 3

    var body: some View {
        GeometryReader { geo in
            let segW = (geo.size.width - pad * 2) / CGFloat(options.count)
            let selIndex = options.firstIndex { $0.id == selected }
            let shown = dragIndex ?? selIndex
            ZStack(alignment: .leading) {
                RoundedRectangle(cornerRadius: 13, style: .continuous).fill(.quaternary.opacity(0.6))
                if let i = shown {
                    Thumb()
                        .frame(width: segW, height: geo.size.height - pad * 2)
                        .scaleEffect(pressed ? 1.06 : 1)
                        .offset(x: pad + CGFloat(i) * segW)
                        .animation(.spring(response: 0.3, dampingFraction: 0.78), value: i)
                        .animation(.spring(response: 0.25, dampingFraction: 0.7), value: pressed)
                }
                HStack(spacing: 0) {
                    ForEach(Array(options.enumerated()), id: \.element.id) { idx, o in
                        let on = idx == shown
                        VStack(spacing: 5) {
                            Image(systemName: o.symbol).font(.system(size: 22)).frame(width: 34, height: 30, alignment: .center)
                            Text(o.label)
                                .font(.system(size: 10.5, weight: .medium))
                                .foregroundStyle(on ? Color.primary : Color.primary.opacity(0.75))
                                .minimumScaleFactor(0.85)
                                .multilineTextAlignment(.center)
                                .lineLimit(2)
                                .frame(height: 28, alignment: .top)
                        }
                        .padding(.vertical, 8)
                        .frame(width: segW, height: geo.size.height - pad * 2)
                    }
                }
                .padding(.horizontal, pad)
                .allowsHitTesting(false)
            }
            .contentShape(Rectangle())
            .gesture(
                DragGesture(minimumDistance: 0)
                    .onChanged { v in
                        pressed = true
                        let i = Int(((v.location.x - pad) / segW).rounded(.down))
                        dragIndex = min(max(i, 0), options.count - 1)
                    }
                    .onEnded { _ in
                        pressed = false
                        if let i = dragIndex, options[i].id != selected { onSelect(options[i].id) }
                        dragIndex = nil
                    }
            )
        }
        .frame(height: 84)
        .opacity(enabled ? 1 : 0.45)
        .disabled(!enabled)
    }
}

struct Section<Content: View>: View {
    let title: String
    @ViewBuilder var content: Content
    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(title.uppercased()).font(.system(size: 11, weight: .semibold)).foregroundStyle(.secondary)
            content
        }
    }
}

struct PickerRow: View {
    let title: String
    let options: [(String, String)]
    let selected: String?
    let enabled: Bool
    let onSelect: (String) -> Void

    var body: some View {
        HStack {
            Text(title).font(.system(size: 13))
            Spacer()
            Menu {
                ForEach(options, id: \.0) { id, label in
                    Button { onSelect(id) } label: {
                        if id == selected { Label(label, systemImage: "checkmark") } else { Text(label) }
                    }
                }
            } label: {
                Text(options.first { $0.0 == selected }?.1 ?? "–").font(.system(size: 13))
            }
            .menuStyle(.button)
            .buttonStyle(.plain)
            .foregroundStyle(.secondary)
            .fixedSize()
        }
        .padding(.horizontal, 12).padding(.vertical, 9)
        .opacity(enabled ? 1 : 0.45).disabled(!enabled)
    }
}

let eqOptions = [("balanced", "Balanced"), ("bold", "Bold"), ("serenade", "Serenade"), ("bassboost", "Bass boost"), ("dynaudio", "Dynaudio")]
let spatialOptions = [("off", "Off", "person.fill"), ("fixed", "Fixed", "person.wave.2.fill"), ("headtracked", "Head Tracked", "person.spatialaudio.fill")]

struct SpatialRow: View {
    let symbol: String
    let label: String
    let selected: Bool
    let enabled: Bool
    let action: () -> Void
    @State private var hover = false

    var body: some View {
        Button(action: action) {
            HStack(spacing: 10) {
                Image(systemName: "checkmark").font(.system(size: 12, weight: .semibold))
                    .frame(width: 16).opacity(selected ? 1 : 0)
                Image(systemName: symbol).font(.system(size: 15)).frame(width: 26)
                Text(label).font(.system(size: 13, weight: selected ? .medium : .regular))
                Spacer()
            }
            .padding(.horizontal, 12).padding(.vertical, 8)
            .background(hover && enabled ? Color.primary.opacity(0.08) : .clear)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .onHover { hover = $0 }
        .opacity(enabled ? 1 : 0.45).disabled(!enabled)
    }
}
let ncLevels = [("high", "High"), ("medium", "Medium"), ("low", "Low")]

let actionLabels: [String: String] = [
    "none": "None", "playpause": "Play/Pause", "previous": "Previous Track", "next": "Next Track",
    "voice": "Voice Assistant", "gamemode": "Game Mode", "volume": "Volume Control", "skiptrack": "Skip Track",
]
let pressChoices = ["none", "playpause", "previous", "next", "voice", "gamemode"]
let swipeChoices = ["none", "volume", "skiptrack"]
let gestureRows: [(id: String, label: String, symbol: String)] = [
    ("press", "Press", "hand.tap"),
    ("double", "Press Twice", "hand.tap.fill"),
    ("triple", "Press 3 Times", "3.circle"),
    ("swipe", "Swipe", "hand.draw"),
]

struct ControlsPage: View {
    @ObservedObject var daemon: Daemon
    let back: () -> Void
    @State private var side = "left"

    var body: some View {
        let s = daemon.state
        let set = side == "left" ? s.gestures?.left : s.gestures?.right
        let hold = s.hold ?? 0
        VStack(alignment: .leading, spacing: 14) {
            Button(action: back) {
                HStack(spacing: 4) {
                    Image(systemName: "chevron.left").font(.system(size: 13, weight: .semibold))
                    Text("Earbud Controls").font(.system(size: 14, weight: .semibold))
                }
            }
            .buttonStyle(.plain)

            Picker("", selection: $side) {
                Text("Left").tag("left")
                Text("Right").tag("right")
            }
            .pickerStyle(.segmented).labelsHidden()

            Section(title: "Press and Hold Earbuds") {
                VStack(spacing: 0) {
                    ForEach(Array(gestureRows.enumerated()), id: \.element.id) { i, row in
                        if i > 0 { Divider().padding(.leading, 12) }
                        PickerRow(
                            title: row.label,
                            options: (row.id == "swipe" ? swipeChoices : pressChoices).map { ($0, actionLabels[$0] ?? $0) },
                            selected: set?.action(row.id),
                            enabled: s.connected && set != nil
                        ) { daemon.setGesture(side: side, gesture: row.id, action: $0) }
                    }
                    Divider().padding(.leading, 12)
                    HoldRow(mask: hold, enabled: s.connected && s.hold != nil) { daemon.setHold($0) }
                }
                .background(RoundedRectangle(cornerRadius: 10, style: .continuous).fill(.quaternary.opacity(0.5)))
            }
            Text("Settings are applied to the selected earbud. Press and Hold cycles through the chosen listening modes on both earbuds.")
                .font(.system(size: 11)).foregroundStyle(.secondary)
        }
    }
}

struct HoldRow: View {
    let mask: Int
    let enabled: Bool
    let onChange: (Int) -> Void
    private let modes: [(Int, String)] = [(2, "Noise Cancellation"), (1, "Off"), (4, "Transparency")]

    var body: some View {
        let summary = modes.filter { mask & $0.0 != 0 }.map { $0.1 == "Noise Cancellation" ? "Noise Cancellation" : $0.1 }.joined(separator: ", ")
        HStack {
            Text("Press and Hold").font(.system(size: 13))
            Spacer()
            Menu {
                ForEach(modes, id: \.0) { bit, label in
                    let on = mask & bit != 0
                    let count = modes.filter { mask & $0.0 != 0 }.count
                    Toggle(label, isOn: Binding(get: { on }, set: { v in onChange(v ? mask | bit : mask & ~bit) }))
                        .disabled(on && count <= 2)
                }
            } label: {
                Text(summary.isEmpty ? "–" : summary).font(.system(size: 13)).lineLimit(1)
            }
            .menuStyle(.button).buttonStyle(.plain).foregroundStyle(.secondary).fixedSize()
        }
        .padding(.horizontal, 12).padding(.vertical, 9)
        .opacity(enabled ? 1 : 0.45).disabled(!enabled)
    }
}

struct ToggleRow: View {
    let title: String
    let detail: String
    let isOn: Bool
    let enabled: Bool
    let onChange: (Bool) -> Void

    var body: some View {
        HStack(spacing: 12) {
            VStack(alignment: .leading, spacing: 2) {
                Text(title).font(.system(size: 13))
                Text(detail).font(.system(size: 11)).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
            }
            Spacer()
            Toggle("", isOn: Binding(get: { isOn }, set: onChange)).toggleStyle(.switch).labelsHidden().controlSize(.small)
        }
        .padding(.horizontal, 12).padding(.vertical, 9)
        .opacity(enabled ? 1 : 0.45).disabled(!enabled)
    }
}

struct SettingsPage: View {
    let back: () -> Void
    @AppStorage("notifyLow") private var notifyLow = false
    @AppStorage("lowThreshold") private var threshold = 20
    @AppStorage("notifyConnection") private var notifyConnection = false
    @AppStorage("menuBarBattery") private var menuBarBattery = false
    @State private var launchAtLogin = SMAppService.mainApp.status == .enabled

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Button(action: back) {
                HStack(spacing: 4) {
                    Image(systemName: "chevron.left").font(.system(size: 13, weight: .semibold))
                    Text("Settings").font(.system(size: 14, weight: .semibold))
                }
            }
            .buttonStyle(.plain)

            Section(title: "Notifications") {
                VStack(spacing: 0) {
                    ToggleRow(title: "Low Battery", detail: "Alert when an earbud or the case runs low.", isOn: notifyLow, enabled: true) { on in
                        notifyLow = on
                        if on { Notifier.requestAuth() }
                    }
                    Divider().padding(.leading, 12)
                    PickerRow(title: "Alert At", options: [10, 15, 20, 30].map { ("\($0)", "\($0)%") }, selected: "\(threshold)", enabled: notifyLow) {
                        threshold = Int($0) ?? 20
                    }
                    Divider().padding(.leading, 12)
                    ToggleRow(title: "Connection", detail: "Alert when the buds connect or disconnect.", isOn: notifyConnection, enabled: true) { on in
                        notifyConnection = on
                        if on { Notifier.requestAuth() }
                    }
                }
                .background(RoundedRectangle(cornerRadius: 10, style: .continuous).fill(.quaternary.opacity(0.5)))
                Button("Send Test Notification") {
                    Notifier.requestAuth { Notifier.post("Buds", "Notifications are working.") }
                }
                .controlSize(.small)
            }

            Section(title: "General") {
                VStack(spacing: 0) {
                    ToggleRow(title: "Launch at Login", detail: "Start Buds when you log in.", isOn: launchAtLogin, enabled: true) { on in
                        do {
                            if on { try SMAppService.mainApp.register() } else { try SMAppService.mainApp.unregister() }
                        } catch {}
                        launchAtLogin = SMAppService.mainApp.status == .enabled
                    }
                    Divider().padding(.leading, 12)
                    ToggleRow(title: "Battery in Menu Bar", detail: "Show left, right and case levels next to the icon.", isOn: menuBarBattery, enabled: true) { menuBarBattery = $0 }
                }
                .background(RoundedRectangle(cornerRadius: 10, style: .continuous).fill(.quaternary.opacity(0.5)))
            }
        }
    }
}

struct Panel: View {
    @ObservedObject var daemon: Daemon
    @State private var showControls = false
    @State private var showSettings = false

    var body: some View {
        Group {
            if showSettings {
                SettingsPage { withAnimation(.snappy(duration: 0.2)) { showSettings = false } }
                    .transition(.move(edge: .trailing).combined(with: .opacity))
            } else if showControls {
                ControlsPage(daemon: daemon) { withAnimation(.snappy(duration: 0.2)) { showControls = false } }
                    .transition(.move(edge: .trailing).combined(with: .opacity))
            } else {
                main.transition(.move(edge: .leading).combined(with: .opacity))
            }
        }
        .padding(16)
        .frame(width: 340)
        .clipped()
    }

    @ViewBuilder var main: some View {
        let s = daemon.state
        let grp = group(of: s.mode)
        VStack(alignment: .leading, spacing: 14) {
            HStack(spacing: 10) {
                Image(systemName: "earbuds").font(.system(size: 20))
                VStack(alignment: .leading, spacing: 1) {
                    Text("OnePlus Buds Pro 3").font(.system(size: 14, weight: .semibold))
                    Text(s.connected ? "Connected" : "Not connected")
                        .font(.system(size: 11)).foregroundStyle(.secondary)
                }
                Spacer()
                if !s.connected {
                    Button("Reconnect") { daemon.send("reconnect") }.controlSize(.small)
                }
            }

            HStack(spacing: 4) {
                BatteryRing(title: "Left", symbol: "airpods.pro.left", cell: s.connected ? s.battery?.left : nil)
                BatteryRing(title: "Right", symbol: "airpods.pro.right", cell: s.connected ? s.battery?.right : nil)
                BatteryRing(title: "Case", symbol: "earbuds.case", cell: s.connected ? s.battery?.case : nil)
            }

            Section(title: "Noise Control") {
                SegmentedIcons(options: modeOptions, selected: grp, enabled: s.connected) { id in
                    daemon.setMode(id == "nc" ? "high" : id)
                }
                if grp == "nc" {
                    Picker("", selection: Binding(get: { s.mode ?? "high" }, set: { daemon.setMode($0) })) {
                        ForEach(ncLevels, id: \.0) { Text($0.1).tag($0.0) }
                    }
                    .pickerStyle(.segmented).labelsHidden().disabled(!s.connected)
                }
            }

            VStack(spacing: 0) {
                PickerRow(title: "Equalizer", options: eqOptions, selected: s.eq, enabled: s.connected) { daemon.setEq($0) }
                Divider().padding(.leading, 12)
                ToggleRow(title: "Game Mode", detail: "Reduces latency for games and video.", isOn: s.game ?? false, enabled: s.connected && s.game != nil) { daemon.setGame($0) }
                Divider().padding(.leading, 12)
                Button { withAnimation(.snappy(duration: 0.2)) { showControls = true } } label: {
                    HStack {
                        Text("Earbud Controls").font(.system(size: 13))
                        Spacer()
                        Image(systemName: "chevron.right").font(.system(size: 11, weight: .semibold)).foregroundStyle(.tertiary)
                    }
                    .padding(.horizontal, 12).padding(.vertical, 9)
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
            }
            .background(RoundedRectangle(cornerRadius: 10, style: .continuous).fill(.quaternary.opacity(0.5)))

            Section(title: "Spatial Audio") {
                VStack(spacing: 0) {
                    ForEach(Array(spatialOptions.enumerated()), id: \.element.0) { i, o in
                        if i > 0 { Divider().padding(.leading, 50) }
                        SpatialRow(symbol: o.2, label: o.1, selected: s.spatial == o.0, enabled: s.connected) {
                            daemon.setSpatial(o.0)
                        }
                    }
                }
                .background(RoundedRectangle(cornerRadius: 10, style: .continuous).fill(.quaternary.opacity(0.5)))
            }

            HStack {
                Button { withAnimation(.snappy(duration: 0.2)) { showSettings = true } } label: {
                    Image(systemName: "gearshape").font(.system(size: 14))
                }
                .buttonStyle(.plain).foregroundStyle(.secondary).help("Settings")
                Spacer()
                Button("Quit") { NSApplication.shared.terminate(nil) }
                    .buttonStyle(.plain).font(.system(size: 12)).foregroundStyle(.secondary)
            }
        }
    }
}

@main
struct BudsApp: App {
    @StateObject private var daemon = Daemon()
    @AppStorage("menuBarBattery") private var menuBarBattery = false

    var body: some Scene {
        MenuBarExtra {
            Panel(daemon: daemon)
        } label: {
            HStack(spacing: 4) {
                Image(systemName: "earbuds").opacity(daemon.state.connected ? 1 : 0.5)
                if menuBarBattery, daemon.state.connected, let b = daemon.state.battery {
                    let parts = [("L", b.left), ("R", b.right), ("C", b.case)].compactMap { label, cell in
                        cell.map { "\(label) \($0.percent)%" }
                    }
                    if !parts.isEmpty { Text(parts.joined(separator: " ")) }
                }
            }
        }
        .menuBarExtraStyle(.window)
    }
}
