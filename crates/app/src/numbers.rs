//! How the UI's platform writes numbers, passed once by the UI: its decimal separator ([`crate::App::set_decimal_separator`]) and
//! its digit grouping ([`crate::App::set_digit_grouping`]); and the number formats the labels use, rounded as the Windows app's .NET
//! formats rounded them, so a label reads as it did before the core wrote it.

/// How the UI's platform writes numbers: its decimal separator ("." or ",", from the user's region), its group separator and its
/// group sizes. ".", "," and groups of 3 until the UI passes its own.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NumberFormat {
    pub decimal_separator: String,
    pub group_separator: String,
    /// The sizes of the digit groups, from the decimal separator leftwards; the last repeats, and a last 0 leaves the remaining
    /// digits ungrouped: .NET's `NumberFormatInfo.NumberGroupSizes` ("NumberFormatInfo.NumberGroupSizes Property", Remarks,
    /// learn.microsoft.com, ms.date 2025-07-01, read 2026-10-27).
    pub group_sizes: Vec<u32>,
}

impl Default for NumberFormat {
    fn default() -> Self {
        Self {
            decimal_separator: ".".to_owned(),
            group_separator: ",".to_owned(),
            group_sizes: vec![3],
        }
    }
}

impl NumberFormat {
    /// `numerator / denominator` with at most one decimal, none when it is 0 ("26.1", "40").
    pub(crate) fn one_decimal(&self, numerator: u64, denominator: u64) -> String {
        let tenths = rounded(numerator * 10, denominator);
        let (whole, tenth) = (tenths / 10, tenths % 10);
        if tenth == 0 {
            whole.to_string()
        } else {
            format!("{whole}{}{tenth}", self.decimal_separator)
        }
    }

    /// A whole number with its digits grouped, as .NET's "N0" writes it ("12,345").
    pub fn grouped(&self, number: u64) -> String {
        let digits = number.to_string();
        let mut groups = Vec::new();
        let mut end = digits.len();
        let mut sizes = self.group_sizes.iter().copied();
        let mut size = sizes.next().unwrap_or(0);
        while end > 0 {
            let take = usize::try_from(size).unwrap_or(usize::MAX);
            if take == 0 || take >= end {
                groups.push(&digits[..end]);
                break;
            }
            groups.push(&digits[end - take..end]);
            end -= take;
            // The last size repeats.
            size = sizes.next().unwrap_or(size);
        }
        groups.reverse();
        groups.join(&self.group_separator)
    }

    /// `value` as a .NET custom format of zeros and hashes writes it, with `min_decimals` "0"s and up to `max_decimals` decimals
    /// in all after the separator ("0.0": 1 and 1; "0.#": 0 and 1; "0": 0 and 0). .NET first takes the value's first `significant`
    /// digits (15 for a double, 7 for a float), then rounds those digits half up at the format's last decimal (dotnet/runtime main,
    /// `Number.Formatting.cs` `FormatFloat` and `RoundNumber`, `Double.cs` and `Single.cs` `MaxPrecisionCustomFormat`, read
    /// 2026-10-27). No group separators: the formats the labels use have none. The labels' numbers are never negative; a negative
    /// one gets a "-" (how .NET writes one that rounds to 0 is not verified).
    pub fn custom(
        &self,
        value: f64,
        significant: usize,
        min_decimals: usize,
        max_decimals: usize,
    ) -> String {
        if !value.is_finite() {
            return String::new();
        }
        let (mut digits, exponent) = significant_digits(value.abs(), significant.max(1));
        // The digits stand for 0.d1d2d3... × 10^point.
        let mut point = exponent + 1;
        // Half up at the last decimal shown.
        let keep = point + i32::try_from(max_decimals).unwrap_or(i32::MAX);
        if keep < 0 {
            digits.clear();
        } else if let Ok(keep) = usize::try_from(keep)
            && keep < digits.len()
        {
            let up = digits[keep] >= 5;
            digits.truncate(keep);
            if up {
                let mut carry = true;
                for digit in digits.iter_mut().rev() {
                    if *digit == 9 {
                        *digit = 0;
                    } else {
                        *digit += 1;
                        carry = false;
                        break;
                    }
                }
                if carry {
                    digits.insert(0, 1);
                    point += 1;
                }
            }
        }
        let digit_at = |index: i32| -> u8 {
            usize::try_from(index)
                .ok()
                .and_then(|index| digits.get(index).copied())
                .unwrap_or(0)
        };
        let mut whole = String::new();
        for index in 0..point.max(0) {
            whole.push(char::from(b'0' + digit_at(index)));
        }
        if whole.is_empty() {
            whole.push('0');
        }
        let mut fraction: Vec<u8> = (0..max_decimals)
            .map(|offset| digit_at(point + i32::try_from(offset).unwrap_or(i32::MAX)))
            .collect();
        while fraction.len() > min_decimals && fraction.last() == Some(&0) {
            fraction.pop();
        }
        let negative = value < 0.0 && (digits.iter().any(|&digit| digit != 0));
        let mut text = String::new();
        if negative {
            text.push('-');
        }
        text.push_str(&whole);
        if !fraction.is_empty() {
            text.push_str(&self.decimal_separator);
            text.extend(fraction.iter().map(|&digit| char::from(b'0' + digit)));
        }
        text
    }
}

/// The first `count` significant digits of `value` (not negative), correctly rounded, and the decimal exponent of the first:
/// 1234.5 → [1, 2, 3, 4, 5], 3. Zero is no digits.
fn significant_digits(value: f64, count: usize) -> (Vec<u8>, i32) {
    if value == 0.0 {
        return (Vec::new(), 0);
    }
    let text = format!("{value:.prec$e}", prec = count - 1);
    let (mantissa, exponent) = text.split_once('e').unwrap_or((&text, "0"));
    let digits = mantissa
        .bytes()
        .filter(u8::is_ascii_digit)
        .map(|digit| digit - b'0')
        .collect();
    (digits, exponent.parse().unwrap_or(0))
}

/// `numerator / denominator` to the nearest whole number, halfway up, as the Windows app's .NET format strings rounded its bitrates
/// ("0" and "0.#" on a double): a custom format takes the double's first 15 significant digits, then rounds those digits half up
/// (dotnet/runtime main, `Number.Formatting.cs` `FormatFloat` and `Double.cs` `MaxPrecisionCustomFormat => 15`,
/// `Number.Formatting.Common.cs` `RoundNumber`, read 2026-10-27). A bitrate in kbit/s or Mbit/s has fewer than 15 significant
/// digits, so those digits are the exact quotient and this integer rounding gives the same.
pub(crate) fn rounded(numerator: u64, denominator: u64) -> u64 {
    (numerator * 2 + denominator) / (denominator * 2)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn comma() -> NumberFormat {
        NumberFormat {
            decimal_separator: ",".to_owned(),
            group_separator: ".".to_owned(),
            ..NumberFormat::default()
        }
    }

    #[test]
    fn digits_are_grouped_as_the_region_groups_them() {
        let format = NumberFormat::default();
        assert_eq!(format.grouped(0), "0");
        assert_eq!(format.grouped(999), "999");
        assert_eq!(format.grouped(1_000), "1,000");
        assert_eq!(format.grouped(12_345_678), "12,345,678");
        assert_eq!(comma().grouped(1_234_567), "1.234.567");
        // The docs' examples, shorter: { 3, 4, 5 } and { 3, 4, 0 }.
        let sizes = |group_sizes: Vec<u32>| NumberFormat {
            group_sizes,
            ..NumberFormat::default()
        };
        assert_eq!(
            sizes(vec![3, 4, 5]).grouped(5555555555554444333),
            "55,55555,55555,4444,333"
        );
        assert_eq!(
            sizes(vec![3, 4, 0]).grouped(5555555554444333),
            "555555555,4444,333"
        );
        // India's groups: 3, then 2.
        assert_eq!(sizes(vec![3, 2]).grouped(123456789), "12,34,56,789");
        assert_eq!(sizes(vec![0]).grouped(1_234_567), "1234567");
        assert_eq!(sizes(Vec::new()).grouped(1_234_567), "1234567");
    }

    #[test]
    fn custom_formats_round_half_up_on_their_significant_digits() {
        let format = NumberFormat::default();
        let one = |value: f64| format.custom(value, 15, 1, 1);
        assert_eq!(one(7.3), "7.3");
        assert_eq!(one(6.25), "6.3");
        assert_eq!(one(6.35), "6.4");
        assert_eq!(one(9.96), "10.0");
        assert_eq!(one(0.0), "0.0");
        assert_eq!(one(0.04), "0.0");
        let up_to_one = |value: f64| format.custom(value, 15, 0, 1);
        assert_eq!(up_to_one(1.0), "1");
        assert_eq!(up_to_one(1.25), "1.3");
        assert_eq!(up_to_one(9.97), "10");
        assert_eq!(format.custom(0.5, 15, 0, 0), "1");
        assert_eq!(format.custom(2.5, 15, 0, 0), "3");
        assert_eq!(format.custom(1023.5, 15, 0, 0), "1024");
        assert_eq!(format.custom(23.976_023_976, 15, 0, 3), "23.976");
        assert_eq!(format.custom(0.000_4, 15, 0, 3), "0");
        assert_eq!(comma().custom(1.5, 15, 0, 1), "1,5");
        // A float's 7 digits: 6.65f32 is 6.6500001..., which rounds up either way; 0.15f32 is 0.150000006, "0.1500000" in 7.
        assert_eq!(format.custom(f64::from(6.65_f32), 7, 1, 1), "6.7");
        assert_eq!(format.custom(f64::from(0.15_f32), 7, 1, 1), "0.2");
        // 15 digits make 0.15 (0.1499999999999999944...) "0.150000000000000", which rounds up.
        assert_eq!(format.custom(0.15, 15, 1, 1), "0.2");
    }
}
