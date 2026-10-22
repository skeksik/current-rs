import Foundation

/// Synthetic subtitle files for the spike.
public enum Generators {
    /// Dialog lines at the bottom, a sign at the top left, a half-transparent line; repeats every 20 s for 60 s.
    public static func dialogPGS(width: Int, height: Int) -> Data {
        var writer = PGS.Writer(width: width, height: height)
        let scale = Double(height) / 1080
        var white = TextBitmap.Style()
        white.size = 56 * scale
        white.outlineWidth = max(2, 4 * scale)
        var sign = white
        sign.fontName = "Georgia-Bold"
        sign.fill = (255, 220, 0)
        sign.outline = (0, 0, 128)
        var half = white
        half.fillAlpha = 128
        half.outline = (0, 0, 0)
        func bottom(_ text: String, _ style: TextBitmap.Style) -> PGS.Object {
            let t = TextBitmap.render(text, style: style)
            return PGS.Object(x: (width - t.w) / 2, y: height - Int(90 * scale) - t.h, w: t.w, h: t.h, indices: t.indices)
        }
        for cycle in 0..<3 {
            let o = Double(cycle * 20)
            writer.show(pts: o + 1, objects: [bottom("PGS bottom line, white with outline", white)], palette: TextBitmap.palette(white))
            writer.clear(pts: o + 4)
            let s = TextBitmap.render("PGS SIGN, TOP LEFT", style: sign)
            // Two objects share one palette here, so both use the sign's colours.
            writer.show(pts: o + 4.5, objects: [PGS.Object(x: Int(120 * scale), y: Int(100 * scale), w: s.w, h: s.h, indices: s.indices),
                                                bottom("Second object, same palette", sign)], palette: TextBitmap.palette(sign))
            writer.clear(pts: o + 8)
            writer.show(pts: o + 8.5, objects: [bottom("Half-transparent fill (alpha 128)", half)], palette: TextBitmap.palette(half))
            writer.clear(pts: o + 12)
            writer.show(pts: o + 12.5, objects: [bottom(String(format: "Clears at %.3f s", o + 16), white)], palette: TextBitmap.palette(white))
            writer.clear(pts: o + 16)
        }
        return writer.data
    }

    /// A deliberately heavy script: about 40 animated events at once (moves, rotation, blur, karaoke, fades, clips).
    public static func busyASS(seconds: Int) -> String {
        var s = """
        [Script Info]
        ScriptType: v4.00+
        PlayResX: 1920
        PlayResY: 1080
        ScaledBorderAndShadow: yes
        WrapStyle: 0

        [V4+ Styles]
        Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding
        Style: Default,Helvetica Neue,58,&H00FFFFFF,&H000000FF,&H00000000,&H80000000,1,0,0,0,100,100,0,0,1,3,2,2,60,60,50,1
        Style: Kara,Avenir Next,48,&H00FFFFFF,&H0000A5FF,&H00402000,&H00000000,1,0,0,0,100,100,0,0,1,3,0,8,40,40,40,1
        Style: Sign,Georgia,64,&H0000E0FF,&H000000FF,&H00600000,&H00000000,1,0,0,0,100,100,0,0,1,4,0,5,0,0,0,1

        [Events]
        Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text

        """
        func t(_ x: Double) -> String {
            let c = Int((x * 100).rounded())
            return String(format: "%d:%02d:%02d.%02d", c / 360_000, c / 6000 % 60, c / 100 % 60, c % 100)
        }
        var i = 0
        var start = 0.0
        while start < Double(seconds) {
            let end = start + 3
            let x = 200 + (i * 173) % 1500, y = 150 + (i * 97) % 700
            // Moving, rotating, blurred sign.
            s += "Dialogue: 0,\(t(start)),\(t(end)),Sign,,0,0,0,,{\\move(\(x),\(y),\(x + 200),\(y + 80))\\blur3\\t(\\frz360\\fscx140\\fscy140)}Sign \(i)\n"
            // Karaoke with fill sweep, at the top.
            s += "Dialogue: 0,\(t(start)),\(t(end)),Kara,,0,0,\(40 + (i % 4) * 60),,{\\fad(200,200)}"
                + "{\\kf40}Ka{\\kf40}ra{\\kf40}o{\\kf40}ke {\\kf40}line {\\kf40}num{\\kf60}ber \(i)\n"
            // Fading, scaling dialogue with a clip and a coloured border.
            s += "Dialogue: 0,\(t(start)),\(t(end)),Default,,0,0,\(50 + (i % 3) * 70),,{\\fad(300,300)\\3c&H\(String(format: "%06X", (i * 0x231F7) & 0xFFFFFF))&"
                + "\\t(0,1500,\\fscx120)\\clip(0,0,1920,\(700 + (i % 5) * 70))\\be2}Dialogue line \(i) with border and shadow\n"
            // A wide blur box behind a sign (expensive: large blur).
            if i % 3 == 0 {
                s += "Dialogue: 0,\(t(start)),\(t(end)),Sign,,0,0,0,,{\\an7\\pos(\(x / 2),\(y / 2))\\p1\\blur12\\1c&H202020&\\alpha&H60&}"
                    + "m 0 0 l 600 0 600 160 0 160{\\p0}\n"
            }
            i += 1
            start += 0.25
        }
        return s
    }

    /// Adds `font` as a `[Fonts]` section (the script format's own uuencoding: 6 bits + 33 per character, 80 per line).
    public static func embed(font: Data, name: String, in script: String) -> String {
        let bytes = [UInt8](font)
        var chars: [UInt8] = []
        var i = 0
        while i < bytes.count {
            let n = min(3, bytes.count - i)
            let b0 = UInt32(bytes[i]), b1 = n > 1 ? UInt32(bytes[i + 1]) : 0, b2 = n > 2 ? UInt32(bytes[i + 2]) : 0
            let v = b0 << 16 | b1 << 8 | b2
            let groups = n == 3 ? 4 : n + 1
            for g in 0..<groups { chars.append(UInt8((v >> (18 - 6 * UInt32(g))) & 0x3f) + 33) }
            i += 3
        }
        var lines: [String] = []
        var p = 0
        while p < chars.count {
            lines.append(String(decoding: chars[p..<min(chars.count, p + 80)], as: UTF8.self))
            p += 80
        }
        let base = (name as NSString).deletingPathExtension, ext = (name as NSString).pathExtension
        let section = "[Fonts]\nfontname: \(base)_0.\(ext)\n" + lines.joined(separator: "\n") + "\n\n"
        // The [Fonts] section goes before [Events].
        guard let range = script.range(of: "[Events]") else { return script + "\n" + section }
        return script.replacingCharacters(in: range, with: section + "[Events]")
    }
}
