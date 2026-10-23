// The network key can name the Wi-Fi network: printing the facts leaves it out. `String(describing:)`, string interpolation and
// `debugPrint` use these; `dump` and `Mirror` still read the stored property, so never dump the facts into a log.

import CurrentFFIBindings

extension NetworkFacts: CustomStringConvertible, CustomDebugStringConvertible {
    public var description: String {
        "NetworkFacts(connectivity: \(connectivity), metered: \(String(describing: metered)), "
            + "constrained: \(String(describing: constrained)), interface: \(interface), vpn: \(String(describing: vpn)), "
            + "networkKey: \(networkKey == nil ? "nil" : "[redacted]"))"
    }

    public var debugDescription: String { description }
}
