import SwiftUI
import AppKit
import Charts
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

func L(_ key: String) -> String { NSLocalizedString(key, comment: "") }

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

struct Sample: Codable, Identifiable, Equatable {
    var t: Double
    var l: Int?
    var r: Int?
    var c: Int?
    var lc: Bool
    var rc: Bool
    var cc: Bool
    var id: Double { t }

    func level(_ side: String) -> (pct: Int, charging: Bool)? {
        switch side {
        case "left": return l.map { ($0, lc) }
        case "right": return r.map { ($0, rc) }
        default: return c.map { ($0, cc) }
        }
    }

    func sameValues(_ o: Sample) -> Bool { l == o.l && r == o.r && c == o.c && lc == o.lc && rc == o.rc && cc == o.cc }
}

final class History: ObservableObject {
    @Published var samples: [Sample] = []
    private let url: URL
    private let keep: TimeInterval = 7 * 24 * 3600

    init() {
        let dir = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Application Support/Buds")
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        url = dir.appendingPathComponent("history.jsonl")
        if let text = try? String(contentsOf: url, encoding: .utf8) {
            let dec = JSONDecoder()
            samples = text.split(separator: "\n").compactMap { try? dec.decode(Sample.self, from: Data($0.utf8)) }
        }
        prune()
    }

    private var lastPrune = Date()

    // Rolling window: drop everything older than `keep` and rewrite the file.
    private func prune() {
        lastPrune = Date()
        let cutoff = Date().timeIntervalSince1970 - keep
        guard let oldest = samples.first, oldest.t < cutoff else { return }
        samples = samples.filter { $0.t >= cutoff }
        let enc = JSONEncoder()
        let text = samples.compactMap { try? enc.encode($0) }.compactMap { String(data: $0, encoding: .utf8) }.joined(separator: "\n")
        try? (text + (text.isEmpty ? "" : "\n")).write(to: url, atomically: true, encoding: .utf8)
    }

    func record(_ state: BudsState) {
        guard state.connected, let b = state.battery, b.left != nil || b.right != nil || b.case != nil else { return }
        if Date().timeIntervalSince(lastPrune) > 3600 { prune() }
        let now = Date().timeIntervalSince1970
        let s = Sample(t: now, l: b.left?.percent, r: b.right?.percent, c: b.case?.percent,
                       lc: b.left?.charging ?? false, rc: b.right?.charging ?? false, cc: b.case?.charging ?? false)
        if let last = samples.last, last.sameValues(s), now - last.t < 300 { return }
        samples.append(s)
        guard let data = try? JSONEncoder().encode(s), var line = String(data: data, encoding: .utf8) else { return }
        line += "\n"
        if let h = try? FileHandle(forWritingTo: url) {
            h.seekToEndOfFile()
            h.write(Data(line.utf8))
            try? h.close()
        } else {
            try? line.write(to: url, atomically: true, encoding: .utf8)
        }
    }
}

final class Daemon: ObservableObject {
    let history = History()
    @Published var state = BudsState() { didSet { evaluate(old: oldValue, new: state); history.record(state) } }
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
            Notifier.post(L(new.connected ? "Buds connected" : "Buds disconnected"), "OnePlus Buds Pro 3")
        }
        guard d.bool(forKey: "notifyLow") else { return }
        let threshold = d.object(forKey: "lowThreshold") as? Int ?? 20
        let cells: [(String, CellState?)] = [("Left earbud", new.battery?.left), ("Right earbud", new.battery?.right), ("Charging case", new.battery?.case)]
        for (name, cell) in cells {
            guard let c = cell else { continue }
            if !c.charging && c.percent <= threshold {
                if lowNotified.insert(name).inserted {
                    Notifier.post(String(format: L("%@ battery low"), L(name)), String(format: L("%ld%% remaining"), c.percent))
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

struct ChargingPulse: ViewModifier {
    let active: Bool
    let color: Color
    var scale: CGFloat = 1

    func body(content: Content) -> some View {
        if active {
            content.phaseAnimator([0.0, 1.0]) { view, p in
                view
                    .opacity(0.72 + 0.28 * p)
                    .scaleEffect(1 + (scale - 1) * p)
                    .shadow(color: color.opacity(0.75 * p), radius: 1 + 5 * p)
            } animation: { _ in .easeInOut(duration: 1.3) }
        } else {
            content
        }
    }
}

struct BatteryRing: View {
    let title: String
    let symbol: String
    let cell: CellState?
    @AppStorage("lowThreshold") private var lowThreshold = 20

    private var isLow: Bool { cell.map { !$0.charging && $0.percent <= lowThreshold } ?? false }

    var body: some View {
        VStack(spacing: 6) {
            ZStack {
                Circle().stroke(Color.primary.opacity(0.22), lineWidth: 6)
                if let c = cell {
                    Circle()
                        .trim(from: 0, to: CGFloat(c.percent) / 100)
                        .stroke(isLow ? Color(red: 1.0, green: 0.27, blue: 0.23) : Color(red: 0.2, green: 0.86, blue: 0.38),
                                style: StrokeStyle(lineWidth: 6, lineCap: .round))
                        .rotationEffect(.degrees(-90))
                        .animation(.easeInOut(duration: 0.6), value: c.percent)
                        .modifier(ChargingPulse(active: c.charging, color: Color(red: 0.2, green: 0.86, blue: 0.38)))
                }
                Image(systemName: symbol).font(.system(size: 17)).foregroundStyle(.primary)
                if cell?.charging == true {
                    Image(systemName: "bolt.fill").font(.system(size: 9)).foregroundStyle(.green)
                        .offset(y: -17).opacity(0)
                }
            }
            .frame(width: 52, height: 52)
            HStack(spacing: 2) {
                if cell?.charging == true {
                    Image(systemName: "bolt.fill").font(.system(size: 9)).foregroundStyle(Color(red: 0.2, green: 0.86, blue: 0.38))
                        .modifier(ChargingPulse(active: true, color: Color(red: 0.2, green: 0.86, blue: 0.38), scale: 1.25))
                }
                Text(cell.map { "\($0.percent)%" } ?? "–").font(.system(size: 12, weight: .medium)).monospacedDigit()
                    .foregroundStyle(isLow ? Color(red: 1.0, green: 0.27, blue: 0.23) : Color.primary)
            }
            Text(L(title)).font(.system(size: 11)).foregroundStyle(.secondary)
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
                            Text(L(o.label))
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
            Text(L(title).uppercased()).font(.system(size: 11, weight: .semibold)).foregroundStyle(.secondary)
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
            Text(L(title)).font(.system(size: 13))
            Spacer()
            Menu {
                ForEach(options, id: \.0) { id, label in
                    Button { onSelect(id) } label: {
                        if id == selected { Label(L(label), systemImage: "checkmark") } else { Text(L(label)) }
                    }
                }
            } label: {
                Text(options.first { $0.0 == selected }.map { L($0.1) } ?? "–").font(.system(size: 13))
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
                Text(L(label)).font(.system(size: 13, weight: selected ? .medium : .regular))
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
            Text("Applies to the selected earbud. Press and Hold cycles listening modes on both.")
                .font(.system(size: 11)).foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
        }
    }
}

struct HoldRow: View {
    let mask: Int
    let enabled: Bool
    let onChange: (Int) -> Void
    private let modes: [(Int, String)] = [(2, "Noise Cancellation"), (1, "Off"), (4, "Transparency")]

    var body: some View {
        let summary = modes.filter { mask & $0.0 != 0 }.map { L($0.1) }.joined(separator: ", ")
        HStack {
            Text("Press and Hold").font(.system(size: 13))
            Spacer()
            Menu {
                ForEach(modes, id: \.0) { bit, label in
                    let on = mask & bit != 0
                    let count = modes.filter { mask & $0.0 != 0 }.count
                    Toggle(L(label), isOn: Binding(get: { on }, set: { v in onChange(v ? mask | bit : mask & ~bit) }))
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
                Text(L(title)).font(.system(size: 13))
                Text(L(detail)).font(.system(size: 11)).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
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
                    Notifier.requestAuth { Notifier.post("Buds", L("Notifications are working.")) }
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

struct BatteryPoint: Identifiable {
    let date: Date
    let pct: Int
    let charging: Bool
    var id: Date { date }
}

struct ChargeSpan: Identifiable {
    let start: Date
    let end: Date
    var id: Date { start }
    var mid: Date { Date(timeIntervalSince1970: (start.timeIntervalSince1970 + end.timeIntervalSince1970) / 2) }
}

struct StatRow: View {
    let title: String
    let value: String

    var body: some View {
        HStack {
            Text(L(title)).font(.system(size: 13))
            Spacer()
            Text(value).font(.system(size: 13)).foregroundStyle(.secondary).monospacedDigit()
        }
        .padding(.horizontal, 12).padding(.vertical, 9)
    }
}

struct StatsPage: View {
    @ObservedObject var history: History
    let back: () -> Void
    @AppStorage("lowThreshold") private var lowThreshold = 20
    @State private var side = "left"
    @State private var hours = 24

    private let green = Color(red: 0.2, green: 0.86, blue: 0.38)
    private let red = Color(red: 1.0, green: 0.27, blue: 0.23)

    private func data() -> (points: [BatteryPoint], spans: [ChargeSpan], bucket: TimeInterval, from: Date, to: Date) {
        let now = Date()
        let from = now.addingTimeInterval(-Double(hours) * 3600)
        let bucket = Double(hours) * 3600 / 96
        var byBucket: [Int: BatteryPoint] = [:]
        for s in history.samples where s.t >= from.timeIntervalSince1970 {
            guard let l = s.level(side) else { continue }
            byBucket[Int(s.t / bucket)] = BatteryPoint(date: Date(timeIntervalSince1970: s.t), pct: l.pct, charging: l.charging)
        }
        let points = byBucket.keys.sorted().compactMap { byBucket[$0] }
        var spans: [ChargeSpan] = []
        var start: Date?
        var prev: Date?
        for p in points {
            if p.charging {
                if start == nil { start = p.date }
                prev = p.date
            } else if let st = start, let pr = prev {
                spans.append(ChargeSpan(start: st, end: pr.addingTimeInterval(bucket)))
                start = nil
            }
        }
        if let st = start, let pr = prev { spans.append(ChargeSpan(start: st, end: pr.addingTimeInterval(bucket))) }
        return (points, spans, bucket, from, now)
    }

    private struct Summary {
        var drainPerHour: Double?
        var chargeSeconds: Double = 0
        var average: Double?
    }

    private func summary() -> Summary {
        let from = Date().addingTimeInterval(-Double(hours) * 3600).timeIntervalSince1970
        let pts = history.samples.filter { $0.t >= from }.compactMap { s in s.level(side).map { (t: s.t, pct: $0.pct, ch: $0.charging) } }
        var out = Summary()
        var drop = 0.0, hrs = 0.0
        for (a, b) in zip(pts, pts.dropFirst()) {
            let dt = b.t - a.t
            guard dt > 0, dt <= 1800 else { continue }
            if !a.ch && !b.ch {
                drop += Double(max(0, a.pct - b.pct))
                hrs += dt / 3600
            } else if a.ch && b.ch {
                out.chargeSeconds += dt
            }
        }
        if hrs >= 0.25, drop > 0 { out.drainPerHour = drop / hrs }
        if !pts.isEmpty { out.average = Double(pts.map(\.pct).reduce(0, +)) / Double(pts.count) }
        return out
    }

    private func duration(_ seconds: Double) -> String {
        let f = DateComponentsFormatter()
        f.allowedUnits = [.day, .hour, .minute]
        f.unitsStyle = .abbreviated
        f.maximumUnitCount = 2
        return f.string(from: max(seconds, 60)) ?? "–"
    }

    var body: some View {
        let d = data()
        let sum = summary()
        VStack(alignment: .leading, spacing: 14) {
            Button(action: back) {
                HStack(spacing: 4) {
                    Image(systemName: "chevron.left").font(.system(size: 13, weight: .semibold))
                    Text("Statistics").font(.system(size: 14, weight: .semibold))
                }
            }
            .buttonStyle(.plain)

            Picker("", selection: $side) {
                Text("Left").tag("left")
                Text("Right").tag("right")
                Text("Case").tag("case")
            }
            .pickerStyle(.segmented).labelsHidden()

            Section(title: "Battery Level") {
                if d.points.isEmpty {
                    Text("No battery history yet. Keep the app running to record it.")
                        .font(.system(size: 12)).foregroundStyle(.secondary)
                        .frame(maxWidth: .infinity, minHeight: 150, alignment: .center)
                        .multilineTextAlignment(.center)
                } else {
                    Chart {
                        ForEach(d.spans) { sp in
                            RectangleMark(xStart: .value("From", sp.start), xEnd: .value("To", sp.end), yStart: .value("Min", -14), yEnd: .value("Max", 100))
                                .foregroundStyle(green.opacity(0.16))
                            RuleMark(xStart: .value("From", sp.start), xEnd: .value("To", sp.end), y: .value("Charging", -8))
                                .lineStyle(StrokeStyle(lineWidth: 3, lineCap: .round))
                                .foregroundStyle(green)
                            PointMark(x: .value("Mid", sp.mid), y: .value("Charging", -8))
                                .opacity(0)
                                .annotation(position: .overlay) {
                                    Image(systemName: "bolt.fill").font(.system(size: 9, weight: .bold)).foregroundStyle(green)
                                        .padding(1).background(Circle().fill(.background))
                                }
                        }
                        ForEach(d.points) { p in
                            BarMark(x: .value("Time", p.date), y: .value("Battery", p.pct), width: .fixed(hours == 24 ? 3 : 2))
                                .foregroundStyle(!p.charging && p.pct <= lowThreshold ? red : green)
                        }
                    }
                    .chartXScale(domain: d.from...d.to)
                    .chartYScale(domain: -14...100)
                    .chartYAxis {
                        AxisMarks(position: .trailing, values: [0, 50, 100]) { v in
                            AxisGridLine()
                            AxisValueLabel { if let n = v.as(Int.self) { Text("\(n)%").font(.system(size: 10)) } }
                        }
                    }
                    .chartXAxis {
                        AxisMarks(values: .automatic(desiredCount: 4)) { _ in
                            AxisGridLine(stroke: StrokeStyle(lineWidth: 0.5, dash: [3, 3]))
                            AxisValueLabel(format: hours == 24 ? .dateTime.hour() : .dateTime.weekday(.abbreviated), anchor: .top)
                        }
                    }
                    .frame(height: 170)
                }
            }

            Picker("", selection: $hours) {
                Text("24 h").tag(24)
                Text("7 days").tag(168)
            }
            .pickerStyle(.segmented).labelsHidden()

            VStack(spacing: 0) {
                StatRow(title: "Average Drain", value: sum.drainPerHour.map { String(format: "%.1f %%/h", $0) } ?? "–")
                Divider().padding(.leading, 12)
                StatRow(title: "Estimated Battery Life", value: sum.drainPerHour.map { duration(100 / $0 * 3600) } ?? "–")
                Divider().padding(.leading, 12)
                StatRow(title: "Time Charging", value: sum.chargeSeconds >= 60 ? duration(sum.chargeSeconds) : "–")
                Divider().padding(.leading, 12)
                StatRow(title: "Average Level", value: sum.average.map { String(format: "%.0f%%", $0) } ?? "–")
            }
            .background(RoundedRectangle(cornerRadius: 10, style: .continuous).fill(.quaternary.opacity(0.5)))
        }
    }
}

struct Panel: View {
    @ObservedObject var daemon: Daemon
    @State private var showControls = false
    @State private var showSettings = false
    @State private var showStats = false

    var body: some View {
        Group {
            if showStats {
                StatsPage(history: daemon.history) { withAnimation(.snappy(duration: 0.2)) { showStats = false } }
                    .transition(.move(edge: .trailing).combined(with: .opacity))
            } else if showSettings {
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
                    Text(L(s.connected ? "Connected" : "Not connected"))
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
                Button { withAnimation(.snappy(duration: 0.2)) { showStats = true } } label: {
                    Image(systemName: "chart.bar").font(.system(size: 14))
                }
                .buttonStyle(.plain).foregroundStyle(.secondary).help("Statistics").padding(.leading, 8)
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
