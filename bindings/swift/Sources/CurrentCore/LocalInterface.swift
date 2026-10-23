// A local interface is the device's own address: printing one leaves the address out. `String(describing:)`, string interpolation
// and `debugPrint` use these; `dump` and `Mirror` still read the stored property, so never dump an interface into a log.

import CurrentFFIBindings

extension LocalInterface: CustomStringConvertible, CustomDebugStringConvertible {
    public var description: String {
        "LocalInterface(address: [redacted], prefixLength: \(prefixLength))"
    }

    public var debugDescription: String { description }
}
