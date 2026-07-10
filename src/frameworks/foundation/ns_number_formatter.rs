/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0.
 */
//!
//! `NSNumberFormatter` - formats numbers into strings and parses strings into
//! numbers.

use crate::frameworks::foundation::ns_string::{from_rust_string, to_rust_string};
use crate::frameworks::foundation::NSUInteger;
use crate::objc::{
    id, msg, nil, objc_classes, release, retain, ClassExports, HostObject, NSZonePtr,
};

/// Apple's NSNumberFormatter behavior modes.
/// <https://developer.apple.com/documentation/foundation/nsnumberformatterbehavior>
///
/// * `NSNumberFormatterBehaviorDefault = 0`
/// * `NSNumberFormatterBehavior10_0    = 1000`
/// * `NSNumberFormatterBehavior10_4    = 1040`
const NS_NUMBER_FORMATTER_BEHAVIOR_10_4: NSUInteger = 1040;

/// Process-wide default formatter behavior, used by
/// `+defaultFormatterBehavior` / `+setDefaultFormatterBehavior:`. Apple
/// documents the runtime default as `NSNumberFormatterBehavior10_4` on
/// modern OS versions.
static DEFAULT_FORMATTER_BEHAVIOR: std::sync::atomic::AtomicU32 =
    std::sync::atomic::AtomicU32::new(NS_NUMBER_FORMATTER_BEHAVIOR_10_4);

#[derive(Default)]
struct NSNumberFormatterHostObject {
    number_style: NSUInteger,
    locale: id,
    grouping_separator: id,
    uses_grouping_separator: bool,
    minimum_fraction_digits: NSUInteger,
    maximum_fraction_digits: NSUInteger,
    /// `NSNumberFormatterBehavior` value. Defaults to
    /// `NSNumberFormatterBehavior10_4` (1040) to match modern iOS.
    formatter_behavior: NSUInteger,
    /// Positive/negative format strings (10.0-style behavior).
    /// These are format strings like `"#,##0.##"` that the 10.0-era
    /// `setPositiveFormat:`/`setNegativeFormat:` API accepts. In the 10.4 API
    /// they are superseded by richer properties, but many older apps (including
    /// those using CSComScore / PlayHaven SDKs that still call
    /// `setPositiveFormat:`) set them. We store the ObjC NSString pointer so
    /// that callers that later read back the value get something sensible.
    positive_format: id,
    negative_format: id,
    /// Prefix/suffix accessors — part of the 10.4-style format API.
    positive_prefix: id,
    positive_suffix: id,
    negative_prefix: id,
    negative_suffix: id,
    /// Whether `numberFromString:` / `stringFromNumber:` should produce
    /// `NSDecimalNumber` rather than plain `NSNumber`.  Most apps leave this
    /// `false`.
    generates_decimal_numbers: bool,
    /// `currencySymbol` — the string used to represent the local currency
    /// when formatting with `NSNumberFormatterCurrencyStyle`.
    /// <https://developer.apple.com/documentation/foundation/numberformatter/currencysymbol>
    /// `nil` means "use the locale's default" (we default to `"$"`, matching
    /// the en_US locale reported elsewhere in touchHLE).
    currency_symbol: id,
    /// `currencyCode` — the ISO 4217 currency code (e.g. `"USD"`).
    /// <https://developer.apple.com/documentation/foundation/numberformatter/currencycode>
    currency_code: id,
    /// `internationalCurrencySymbol`.
    /// <https://developer.apple.com/documentation/foundation/numberformatter/internationalcurrencysymbol>
    international_currency_symbol: id,
}
impl HostObject for NSNumberFormatterHostObject {}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation NSNumberFormatter : NSObject

// =========================================================================
// MARK: - Class methods
// =========================================================================

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = NSNumberFormatterHostObject {
        number_style: 0,
        locale: nil,
        grouping_separator: nil,
        uses_grouping_separator: false,
        minimum_fraction_digits: 0,
        maximum_fraction_digits: 0,
        formatter_behavior: NS_NUMBER_FORMATTER_BEHAVIOR_10_4,
        positive_format: nil,
        negative_format: nil,
        positive_prefix: nil,
        positive_suffix: nil,
        negative_prefix: nil,
        negative_suffix: nil,
        generates_decimal_numbers: false,
        currency_symbol: nil,
        currency_code: nil,
        international_currency_symbol: nil,
    };
    env.objc.alloc_object(this, Box::new(host_object), &mut env.mem)
}

// `+ (NSNumberFormatterBehavior)defaultFormatterBehavior`
// <https://developer.apple.com/documentation/foundation/nsnumberformatter/1409014-defaultformatterbehavior>
+ (NSUInteger)defaultFormatterBehavior {
    DEFAULT_FORMATTER_BEHAVIOR.load(std::sync::atomic::Ordering::Relaxed) as NSUInteger
}

// `+ (void)setDefaultFormatterBehavior:(NSNumberFormatterBehavior)behavior`
// <https://developer.apple.com/documentation/foundation/nsnumberformatter/1407959-setdefaultformatterbehavior>
+ (())setDefaultFormatterBehavior:(NSUInteger)behavior {
    DEFAULT_FORMATTER_BEHAVIOR.store(behavior as u32, std::sync::atomic::Ordering::Relaxed);
}

// =========================================================================
// MARK: - Instance methods
// =========================================================================

- (id)init {
    this
}

- (())dealloc {
    let host = env.objc.borrow::<NSNumberFormatterHostObject>(this);
    let ids_to_release = [
        host.locale,
        host.positive_format,
        host.negative_format,
        host.positive_prefix,
        host.positive_suffix,
        host.negative_prefix,
        host.negative_suffix,
        host.currency_symbol,
        host.currency_code,
        host.international_currency_symbol,
    ];
    let _ = host;
    for id in ids_to_release {
        release(env, id);
    }
    env.objc.dealloc_object(this, &mut env.mem)
}

// `- (NSNumberFormatterBehavior)formatterBehavior`
// <https://developer.apple.com/documentation/foundation/nsnumberformatter/1411915-formatterbehavior>
- (NSUInteger)formatterBehavior {
    env.objc.borrow::<NSNumberFormatterHostObject>(this).formatter_behavior
}

// `- (void)setFormatterBehavior:(NSNumberFormatterBehavior)behavior`
// <https://developer.apple.com/documentation/foundation/nsnumberformatter/1416550-setformatterbehavior>
//
// Apple defines three valid values: `NSNumberFormatterBehaviorDefault`
// (0), `NSNumberFormatterBehavior10_0` (1000) and
// `NSNumberFormatterBehavior10_4` (1040). The "default" value is
// documented to be mapped onto the current
// `+defaultFormatterBehavior` (i.e. the 10.4 behaviour on modern
// systems), so store the resolved value to keep `-formatterBehavior`
// observable from the guest.
- (())setFormatterBehavior:(NSUInteger)behavior {
    let resolved = if behavior == 0 {
        DEFAULT_FORMATTER_BEHAVIOR.load(std::sync::atomic::Ordering::Relaxed) as NSUInteger
    } else {
        behavior
    };
    env.objc.borrow_mut::<NSNumberFormatterHostObject>(this).formatter_behavior = resolved;
}

- (NSUInteger)numberStyle {
    env.objc.borrow::<NSNumberFormatterHostObject>(this).number_style
}

- (())setNumberStyle:(NSUInteger)style {
    env.objc.borrow_mut::<NSNumberFormatterHostObject>(this).number_style = style;
}

- (id)locale {
    env.objc.borrow::<NSNumberFormatterHostObject>(this).locale
}

- (())setLocale:(id)locale {
    env.objc.borrow_mut::<NSNumberFormatterHostObject>(this).locale = locale;
}

- (id)groupingSeparator {
    env.objc.borrow::<NSNumberFormatterHostObject>(this).grouping_separator
}

- (())setGroupingSeparator:(id)separator {
    env.objc.borrow_mut::<NSNumberFormatterHostObject>(this).grouping_separator = separator;
}

- (bool)usesGroupingSeparator {
    env.objc.borrow::<NSNumberFormatterHostObject>(this).uses_grouping_separator
}

- (())setUsesGroupingSeparator:(bool)uses {
    env.objc.borrow_mut::<NSNumberFormatterHostObject>(this).uses_grouping_separator = uses;
}

- (NSUInteger)minimumFractionDigits {
    env.objc.borrow::<NSNumberFormatterHostObject>(this).minimum_fraction_digits
}

- (())setMinimumFractionDigits:(NSUInteger)digits {
    env.objc.borrow_mut::<NSNumberFormatterHostObject>(this).minimum_fraction_digits = digits;
}

- (NSUInteger)maximumFractionDigits {
    env.objc.borrow::<NSNumberFormatterHostObject>(this).maximum_fraction_digits
}

- (())setMaximumFractionDigits:(NSUInteger)digits {
    env.objc.borrow_mut::<NSNumberFormatterHostObject>(this).maximum_fraction_digits = digits;
}

- (id)stringFromNumber:(id)number {
    if number == nil {
        return nil;
    }

    let val: f64 = msg![env; number doubleValue];
    let host_obj = env.objc.borrow::<NSNumberFormatterHostObject>(this);
    let style = host_obj.number_style;
    let currency_symbol = host_obj.currency_symbol;

    let rust_string: String;

    // 0 = NoStyle, 1 = DecimalStyle, 2 = CurrencyStyle, 3 = PercentStyle, 4 =
    // ScientificStyle
    if style == 2 {
        // Per Apple's NSNumberFormatter documentation, the currency style
        // uses `currencySymbol` (which defaults to the locale's symbol) as
        // the prefix.
        let symbol = if currency_symbol != nil {
            to_rust_string(env, currency_symbol).to_string()
        } else {
            "$".to_string()
        };
        rust_string = format!("{}{:.2}", symbol, val);
    } else if style == 3 {
        rust_string = format!("{}%", val * 100.0);
    } else if style == 4 {
        rust_string = format!("{:e}", val);
    } else {
        rust_string = format!("{}", val);
    }

    from_rust_string(env, rust_string)
}

- (id)numberFromString:(id)string {
    if string == nil {
        return nil;
    }

    let rust_str = to_rust_string(env, string).to_string();

    // Clean string from currency and percentage signs, including any custom
    // currency symbol configured via setCurrencySymbol:.
    let custom_symbol = env.objc.borrow::<NSNumberFormatterHostObject>(this).currency_symbol;
    let rust_str = if custom_symbol != nil {
        let symbol = to_rust_string(env, custom_symbol).to_string();
        if symbol.is_empty() {
            rust_str
        } else {
            rust_str.replace(&symbol, "")
        }
    } else {
        rust_str
    };
    let clean_str = rust_str.replace(['$', ',', '%'], "");
    let trimmed = clean_str.trim();

    if let Ok(val) = trimmed.parse::<f64>() {
        let ns_number_class = env.objc.get_known_class("NSNumber", &mut env.mem);
        msg![env; ns_number_class numberWithDouble:val]
    } else {
        log!("Warning: NSNumberFormatter failed to parse string '{}'", rust_str);
        nil
    }
}

// =========================================================================
// MARK: - Format string accessors (10.0-style API)
// =========================================================================

// `- (NSString *)positiveFormat`
// <https://developer.apple.com/documentation/foundation/nsnumberformatter/1408157-positiveformat>
- (id)positiveFormat {
    env.objc.borrow::<NSNumberFormatterHostObject>(this).positive_format
}

// `- (void)setPositiveFormat:(NSString *)format`
// <https://developer.apple.com/documentation/foundation/nsnumberformatter/1408157-positiveformat>
- (())setPositiveFormat:(id)format {
    env.objc.borrow_mut::<NSNumberFormatterHostObject>(this).positive_format = format;
}

// `- (NSString *)negativeFormat`
// <https://developer.apple.com/documentation/foundation/nsnumberformatter/1408953-negativeformat>
- (id)negativeFormat {
    env.objc.borrow::<NSNumberFormatterHostObject>(this).negative_format
}

// `- (void)setNegativeFormat:(NSString *)format`
// <https://developer.apple.com/documentation/foundation/nsnumberformatter/1408953-negativeformat>
- (())setNegativeFormat:(id)format {
    env.objc.borrow_mut::<NSNumberFormatterHostObject>(this).negative_format = format;
}

// =========================================================================
// MARK: - Prefix/Suffix accessors
// =========================================================================

// `- (NSString *)positivePrefix`
// <https://developer.apple.com/documentation/foundation/nsnumberformatter/1414464-positiveprefix>
- (id)positivePrefix {
    env.objc.borrow::<NSNumberFormatterHostObject>(this).positive_prefix
}
- (())setPositivePrefix:(id)prefix {
    env.objc.borrow_mut::<NSNumberFormatterHostObject>(this).positive_prefix = prefix;
}

// `- (NSString *)positiveSuffix`
// <https://developer.apple.com/documentation/foundation/nsnumberformatter/1413740-positivesuffix>
- (id)positiveSuffix {
    env.objc.borrow::<NSNumberFormatterHostObject>(this).positive_suffix
}
- (())setPositiveSuffix:(id)suffix {
    env.objc.borrow_mut::<NSNumberFormatterHostObject>(this).positive_suffix = suffix;
}

// `- (NSString *)negativePrefix`
// <https://developer.apple.com/documentation/foundation/nsnumberformatter/1410408-negativeprefix>
- (id)negativePrefix {
    env.objc.borrow::<NSNumberFormatterHostObject>(this).negative_prefix
}
- (())setNegativePrefix:(id)prefix {
    env.objc.borrow_mut::<NSNumberFormatterHostObject>(this).negative_prefix = prefix;
}

// `- (NSString *)negativeSuffix`
// <https://developer.apple.com/documentation/foundation/nsnumberformatter/1413203-negativesuffix>
- (id)negativeSuffix {
    env.objc.borrow::<NSNumberFormatterHostObject>(this).negative_suffix
}
- (())setNegativeSuffix:(id)suffix {
    env.objc.borrow_mut::<NSNumberFormatterHostObject>(this).negative_suffix = suffix;
}

// =========================================================================
// MARK: - Decimal number generation
// =========================================================================

// `- (BOOL)generatesDecimalNumbers`
// <https://developer.apple.com/documentation/foundation/nsnumberformatter/1412019-generatesdecimalnumbers>
- (bool)generatesDecimalNumbers {
    env.objc.borrow::<NSNumberFormatterHostObject>(this).generates_decimal_numbers
}
- (())setGeneratesDecimalNumbers:(bool)flag {
    env.objc.borrow_mut::<NSNumberFormatterHostObject>(this).generates_decimal_numbers = flag;
}

// =========================================================================
// MARK: - Currency configuration
// =========================================================================

// `- (NSString *)currencySymbol`
// <https://developer.apple.com/documentation/foundation/numberformatter/currencysymbol>
//
// Per Apple's documentation this is "the string used by the receiver as a
// local currency symbol"; when the app has not set one, the locale's symbol
// is used. touchHLE reports an en_US-like locale, so the default is "$".
- (id)currencySymbol {
    let symbol = env.objc.borrow::<NSNumberFormatterHostObject>(this).currency_symbol;
    if symbol != nil {
        symbol
    } else {
        from_rust_string(env, "$".to_string())
    }
}

// `- (void)setCurrencySymbol:(NSString *)symbol`
// <https://developer.apple.com/documentation/foundation/numberformatter/currencysymbol>
- (())setCurrencySymbol:(id)symbol {
    retain(env, symbol);
    let old = std::mem::replace(
        &mut env.objc.borrow_mut::<NSNumberFormatterHostObject>(this).currency_symbol,
        symbol,
    );
    release(env, old);
}

// `- (NSString *)currencyCode`
// <https://developer.apple.com/documentation/foundation/numberformatter/currencycode>
- (id)currencyCode {
    let code = env.objc.borrow::<NSNumberFormatterHostObject>(this).currency_code;
    if code != nil {
        code
    } else {
        from_rust_string(env, "USD".to_string())
    }
}

// `- (void)setCurrencyCode:(NSString *)code`
// <https://developer.apple.com/documentation/foundation/numberformatter/currencycode>
- (())setCurrencyCode:(id)code {
    retain(env, code);
    let old = std::mem::replace(
        &mut env.objc.borrow_mut::<NSNumberFormatterHostObject>(this).currency_code,
        code,
    );
    release(env, old);
}

// `- (NSString *)internationalCurrencySymbol`
// <https://developer.apple.com/documentation/foundation/numberformatter/internationalcurrencysymbol>
- (id)internationalCurrencySymbol {
    let symbol =
        env.objc.borrow::<NSNumberFormatterHostObject>(this).international_currency_symbol;
    if symbol != nil {
        symbol
    } else {
        from_rust_string(env, "USD".to_string())
    }
}

// `- (void)setInternationalCurrencySymbol:(NSString *)symbol`
// <https://developer.apple.com/documentation/foundation/numberformatter/internationalcurrencysymbol>
- (())setInternationalCurrencySymbol:(id)symbol {
    retain(env, symbol);
    let old = std::mem::replace(
        &mut env
            .objc
            .borrow_mut::<NSNumberFormatterHostObject>(this)
            .international_currency_symbol,
        symbol,
    );
    release(env, old);
}

@end

};
