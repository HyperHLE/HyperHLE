/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0.
 */
//!
//! `NSNumberFormatter` - formats numbers into strings and parses strings into
//! numbers.

use crate::frameworks::foundation::ns_string::{from_rust_string, to_rust_string};
use crate::frameworks::foundation::NSUInteger;
use crate::objc::{id, msg, nil, objc_classes, release, ClassExports, HostObject, NSZonePtr};

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
    // -----------------------------------------------------------------------
    // Currency / symbol properties
    // Per Apple's NSNumberFormatter documentation:
    // https://developer.apple.com/documentation/foundation/nsnumberformatter
    // -----------------------------------------------------------------------
    /// `currencySymbol` — e.g. "$". Default nil means use the locale's symbol.
    currency_symbol: id,
    /// `currencyCode` — ISO 4217 code, e.g. "USD".
    currency_code: id,
    /// `internationalCurrencySymbol` — e.g. "USD".
    international_currency_symbol: id,
    /// `currencyDecimalSeparator` — decimal separator used in currency style.
    currency_decimal_separator: id,
    /// `currencyGroupingSeparator` — grouping separator used in currency style.
    currency_grouping_separator: id,
    // -----------------------------------------------------------------------
    // Decimal / sign symbols
    // -----------------------------------------------------------------------
    /// `decimalSeparator` — e.g. ".".
    decimal_separator: id,
    /// `alwaysShowsDecimalSeparator` — if true, always show the decimal point.
    always_shows_decimal_separator: bool,
    /// `notANumberSymbol` — symbol used for NaN.
    not_a_number_symbol: id,
    /// `plusSign` — the "+" sign character.
    plus_sign: id,
    /// `minusSign` — the "−" sign character.
    minus_sign: id,
    /// `paddingCharacter` — character used to pad the formatted string.
    padding_character: id,
    /// `percentSymbol` — symbol used for percent style.
    percent_symbol: id,
    /// `perMillSymbol` — symbol used for per-mille (‰).
    per_mill_symbol: id,
    /// `zeroSymbol` — symbol substituted for the value zero.
    zero_symbol: id,
    /// `nilSymbol` — symbol substituted for nil.
    nil_symbol: id,
    // -----------------------------------------------------------------------
    // Integer digit counts
    // -----------------------------------------------------------------------
    /// `maximumIntegerDigits`
    maximum_integer_digits: NSUInteger,
    /// `minimumIntegerDigits`
    minimum_integer_digits: NSUInteger,
    /// `maximumSignificantDigits`
    maximum_significant_digits: NSUInteger,
    /// `minimumSignificantDigits`
    minimum_significant_digits: NSUInteger,
    /// `usesSignificantDigits`
    uses_significant_digits: bool,
    // -----------------------------------------------------------------------
    // Rounding / multiplier
    // -----------------------------------------------------------------------
    /// `roundingMode` — NSNumberFormatterRoundingMode (0–7).
    rounding_mode: NSUInteger,
    /// `roundingIncrement` — value to which results are rounded, or nil.
    rounding_increment: id,
    /// `multiplier` — applied before formatting; nil means no multiplier.
    multiplier: id,
    // -----------------------------------------------------------------------
    // Grouping
    // -----------------------------------------------------------------------
    /// `groupingSize` — number of digits per group.
    grouping_size: NSUInteger,
    /// `secondaryGroupingSize`
    secondary_grouping_size: NSUInteger,
    // -----------------------------------------------------------------------
    // Padding
    // -----------------------------------------------------------------------
    /// `paddingPosition` — NSNumberFormatterPadPosition (0–3).
    padding_position: NSUInteger,
    // -----------------------------------------------------------------------
    // Miscellaneous
    // -----------------------------------------------------------------------
    /// `isLenient` — whether the formatter is lenient when parsing.
    is_lenient: bool,
    /// `isPartialStringValidationEnabled`
    partial_string_validation_enabled: bool,
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
        currency_decimal_separator: nil,
        currency_grouping_separator: nil,
        decimal_separator: nil,
        always_shows_decimal_separator: false,
        not_a_number_symbol: nil,
        plus_sign: nil,
        minus_sign: nil,
        padding_character: nil,
        percent_symbol: nil,
        per_mill_symbol: nil,
        zero_symbol: nil,
        nil_symbol: nil,
        maximum_integer_digits: 42,
        minimum_integer_digits: 1,
        maximum_significant_digits: 0,
        minimum_significant_digits: 0,
        uses_significant_digits: false,
        rounding_mode: 0,
        rounding_increment: nil,
        multiplier: nil,
        grouping_size: 3,
        secondary_grouping_size: 0,
        padding_position: 0,
        is_lenient: false,
        partial_string_validation_enabled: false,
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
        host.currency_decimal_separator,
        host.currency_grouping_separator,
        host.decimal_separator,
        host.not_a_number_symbol,
        host.plus_sign,
        host.minus_sign,
        host.padding_character,
        host.percent_symbol,
        host.per_mill_symbol,
        host.zero_symbol,
        host.nil_symbol,
        host.rounding_increment,
        host.multiplier,
    ];
    drop(host);
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

    let rust_string: String;

    // 0 = NoStyle, 1 = DecimalStyle, 2 = CurrencyStyle, 3 = PercentStyle, 4 =
    // ScientificStyle
    if style == 2 {
        rust_string = format!("${:.2}", val);
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

    let rust_str = to_rust_string(env, string);

    // Clean string from currency and percentage signs
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
// MARK: - Currency symbols
// Per Apple: https://developer.apple.com/documentation/foundation/nsnumberformatter
// =========================================================================

// `- (NSString *)currencySymbol`
// <https://developer.apple.com/documentation/foundation/nsnumberformatter/1407931-currencysymbol>
- (id)currencySymbol {
    env.objc.borrow::<NSNumberFormatterHostObject>(this).currency_symbol
}
// `- (void)setCurrencySymbol:(NSString *)symbol`
- (())setCurrencySymbol:(id)symbol {
    env.objc.borrow_mut::<NSNumberFormatterHostObject>(this).currency_symbol = symbol;
}

// `- (NSString *)currencyCode`
// <https://developer.apple.com/documentation/foundation/nsnumberformatter/1416145-currencycode>
- (id)currencyCode {
    env.objc.borrow::<NSNumberFormatterHostObject>(this).currency_code
}
// `- (void)setCurrencyCode:(NSString *)code`
- (())setCurrencyCode:(id)code {
    env.objc.borrow_mut::<NSNumberFormatterHostObject>(this).currency_code = code;
}

// `- (NSString *)internationalCurrencySymbol`
// <https://developer.apple.com/documentation/foundation/nsnumberformatter/1418485-internationalcurrencysymbol>
- (id)internationalCurrencySymbol {
    env.objc.borrow::<NSNumberFormatterHostObject>(this).international_currency_symbol
}
- (())setInternationalCurrencySymbol:(id)symbol {
    env.objc.borrow_mut::<NSNumberFormatterHostObject>(this).international_currency_symbol = symbol;
}

// `- (NSString *)currencyDecimalSeparator`
// <https://developer.apple.com/documentation/foundation/nsnumberformatter/1412167-currencydecimalseparator>
- (id)currencyDecimalSeparator {
    env.objc.borrow::<NSNumberFormatterHostObject>(this).currency_decimal_separator
}
- (())setCurrencyDecimalSeparator:(id)sep {
    env.objc.borrow_mut::<NSNumberFormatterHostObject>(this).currency_decimal_separator = sep;
}

// `- (NSString *)currencyGroupingSeparator`
// <https://developer.apple.com/documentation/foundation/nsnumberformatter/1416533-currencygroupingseparator>
- (id)currencyGroupingSeparator {
    env.objc.borrow::<NSNumberFormatterHostObject>(this).currency_grouping_separator
}
- (())setCurrencyGroupingSeparator:(id)sep {
    env.objc.borrow_mut::<NSNumberFormatterHostObject>(this).currency_grouping_separator = sep;
}

// =========================================================================
// MARK: - Decimal separator / sign symbols
// =========================================================================

// `- (NSString *)decimalSeparator`
// <https://developer.apple.com/documentation/foundation/nsnumberformatter/1411567-decimalseparator>
- (id)decimalSeparator {
    env.objc.borrow::<NSNumberFormatterHostObject>(this).decimal_separator
}
- (())setDecimalSeparator:(id)sep {
    env.objc.borrow_mut::<NSNumberFormatterHostObject>(this).decimal_separator = sep;
}

// `- (BOOL)alwaysShowsDecimalSeparator`
// <https://developer.apple.com/documentation/foundation/nsnumberformatter/1407941-alwaysshowsdecimalseparator>
- (bool)alwaysShowsDecimalSeparator {
    env.objc.borrow::<NSNumberFormatterHostObject>(this).always_shows_decimal_separator
}
- (())setAlwaysShowsDecimalSeparator:(bool)flag {
    env.objc.borrow_mut::<NSNumberFormatterHostObject>(this).always_shows_decimal_separator = flag;
}

// `- (NSString *)notANumberSymbol`
// <https://developer.apple.com/documentation/foundation/nsnumberformatter/1411229-notanumbersymbol>
- (id)notANumberSymbol {
    env.objc.borrow::<NSNumberFormatterHostObject>(this).not_a_number_symbol
}
- (())setNotANumberSymbol:(id)sym {
    env.objc.borrow_mut::<NSNumberFormatterHostObject>(this).not_a_number_symbol = sym;
}

// `- (NSString *)plusSign`
// <https://developer.apple.com/documentation/foundation/nsnumberformatter/1414467-plussign>
- (id)plusSign {
    env.objc.borrow::<NSNumberFormatterHostObject>(this).plus_sign
}
- (())setPlusSign:(id)sign {
    env.objc.borrow_mut::<NSNumberFormatterHostObject>(this).plus_sign = sign;
}

// `- (NSString *)minusSign`
// <https://developer.apple.com/documentation/foundation/nsnumberformatter/1415845-minussign>
- (id)minusSign {
    env.objc.borrow::<NSNumberFormatterHostObject>(this).minus_sign
}
- (())setMinusSign:(id)sign {
    env.objc.borrow_mut::<NSNumberFormatterHostObject>(this).minus_sign = sign;
}

// `- (NSString *)paddingCharacter`
// <https://developer.apple.com/documentation/foundation/nsnumberformatter/1408530-paddingcharacter>
- (id)paddingCharacter {
    env.objc.borrow::<NSNumberFormatterHostObject>(this).padding_character
}
- (())setPaddingCharacter:(id)ch {
    env.objc.borrow_mut::<NSNumberFormatterHostObject>(this).padding_character = ch;
}

// `- (NSString *)percentSymbol`
// <https://developer.apple.com/documentation/foundation/nsnumberformatter/1408168-percentsymbol>
- (id)percentSymbol {
    env.objc.borrow::<NSNumberFormatterHostObject>(this).percent_symbol
}
- (())setPercentSymbol:(id)sym {
    env.objc.borrow_mut::<NSNumberFormatterHostObject>(this).percent_symbol = sym;
}

// `- (NSString *)perMillSymbol`
// <https://developer.apple.com/documentation/foundation/nsnumberformatter/1418661-permillsymbol>
- (id)perMillSymbol {
    env.objc.borrow::<NSNumberFormatterHostObject>(this).per_mill_symbol
}
- (())setPerMillSymbol:(id)sym {
    env.objc.borrow_mut::<NSNumberFormatterHostObject>(this).per_mill_symbol = sym;
}

// `- (NSString *)zeroSymbol`
// <https://developer.apple.com/documentation/foundation/nsnumberformatter/1414149-zerosymbol>
- (id)zeroSymbol {
    env.objc.borrow::<NSNumberFormatterHostObject>(this).zero_symbol
}
- (())setZeroSymbol:(id)sym {
    env.objc.borrow_mut::<NSNumberFormatterHostObject>(this).zero_symbol = sym;
}

// `- (NSString *)nilSymbol`
// <https://developer.apple.com/documentation/foundation/nsnumberformatter/1415353-nilsymbol>
- (id)nilSymbol {
    env.objc.borrow::<NSNumberFormatterHostObject>(this).nil_symbol
}
- (())setNilSymbol:(id)sym {
    env.objc.borrow_mut::<NSNumberFormatterHostObject>(this).nil_symbol = sym;
}

// =========================================================================
// MARK: - Integer digit counts
// =========================================================================

// `- (NSUInteger)maximumIntegerDigits`
// <https://developer.apple.com/documentation/foundation/nsnumberformatter/1415045-maximumintegerdigits>
- (NSUInteger)maximumIntegerDigits {
    env.objc.borrow::<NSNumberFormatterHostObject>(this).maximum_integer_digits
}
- (())setMaximumIntegerDigits:(NSUInteger)digits {
    env.objc.borrow_mut::<NSNumberFormatterHostObject>(this).maximum_integer_digits = digits;
}

// `- (NSUInteger)minimumIntegerDigits`
// <https://developer.apple.com/documentation/foundation/nsnumberformatter/1417149-minimumintegerdigits>
- (NSUInteger)minimumIntegerDigits {
    env.objc.borrow::<NSNumberFormatterHostObject>(this).minimum_integer_digits
}
- (())setMinimumIntegerDigits:(NSUInteger)digits {
    env.objc.borrow_mut::<NSNumberFormatterHostObject>(this).minimum_integer_digits = digits;
}

// `- (NSUInteger)maximumSignificantDigits`
// <https://developer.apple.com/documentation/foundation/nsnumberformatter/1415672-maximumsignificantdigits>
- (NSUInteger)maximumSignificantDigits {
    env.objc.borrow::<NSNumberFormatterHostObject>(this).maximum_significant_digits
}
- (())setMaximumSignificantDigits:(NSUInteger)digits {
    env.objc.borrow_mut::<NSNumberFormatterHostObject>(this).maximum_significant_digits = digits;
}

// `- (NSUInteger)minimumSignificantDigits`
// <https://developer.apple.com/documentation/foundation/nsnumberformatter/1416365-minimumsignificantdigits>
- (NSUInteger)minimumSignificantDigits {
    env.objc.borrow::<NSNumberFormatterHostObject>(this).minimum_significant_digits
}
- (())setMinimumSignificantDigits:(NSUInteger)digits {
    env.objc.borrow_mut::<NSNumberFormatterHostObject>(this).minimum_significant_digits = digits;
}

// `- (BOOL)usesSignificantDigits`
// <https://developer.apple.com/documentation/foundation/nsnumberformatter/1408598-usessignificantdigits>
- (bool)usesSignificantDigits {
    env.objc.borrow::<NSNumberFormatterHostObject>(this).uses_significant_digits
}
- (())setUsesSignificantDigits:(bool)flag {
    env.objc.borrow_mut::<NSNumberFormatterHostObject>(this).uses_significant_digits = flag;
}

// =========================================================================
// MARK: - Rounding
// =========================================================================

// `- (NSNumberFormatterRoundingMode)roundingMode`
// <https://developer.apple.com/documentation/foundation/nsnumberformatter/1413368-roundingmode>
- (NSUInteger)roundingMode {
    env.objc.borrow::<NSNumberFormatterHostObject>(this).rounding_mode
}
- (())setRoundingMode:(NSUInteger)mode {
    env.objc.borrow_mut::<NSNumberFormatterHostObject>(this).rounding_mode = mode;
}

// `- (NSNumber *)roundingIncrement`
// <https://developer.apple.com/documentation/foundation/nsnumberformatter/1412444-roundingincrement>
- (id)roundingIncrement {
    env.objc.borrow::<NSNumberFormatterHostObject>(this).rounding_increment
}
- (())setRoundingIncrement:(id)inc {
    env.objc.borrow_mut::<NSNumberFormatterHostObject>(this).rounding_increment = inc;
}

// `- (NSNumber *)multiplier`
// <https://developer.apple.com/documentation/foundation/nsnumberformatter/1410128-multiplier>
- (id)multiplier {
    env.objc.borrow::<NSNumberFormatterHostObject>(this).multiplier
}
- (())setMultiplier:(id)mul {
    env.objc.borrow_mut::<NSNumberFormatterHostObject>(this).multiplier = mul;
}

// =========================================================================
// MARK: - Grouping
// =========================================================================

// `- (NSUInteger)groupingSize`
// <https://developer.apple.com/documentation/foundation/nsnumberformatter/1416308-groupingsize>
- (NSUInteger)groupingSize {
    env.objc.borrow::<NSNumberFormatterHostObject>(this).grouping_size
}
- (())setGroupingSize:(NSUInteger)size {
    env.objc.borrow_mut::<NSNumberFormatterHostObject>(this).grouping_size = size;
}

// `- (NSUInteger)secondaryGroupingSize`
// <https://developer.apple.com/documentation/foundation/nsnumberformatter/1408618-secondarygroupingsize>
- (NSUInteger)secondaryGroupingSize {
    env.objc.borrow::<NSNumberFormatterHostObject>(this).secondary_grouping_size
}
- (())setSecondaryGroupingSize:(NSUInteger)size {
    env.objc.borrow_mut::<NSNumberFormatterHostObject>(this).secondary_grouping_size = size;
}

// =========================================================================
// MARK: - Padding
// =========================================================================

// `- (NSNumberFormatterPadPosition)paddingPosition`
// <https://developer.apple.com/documentation/foundation/nsnumberformatter/1413029-paddingposition>
- (NSUInteger)paddingPosition {
    env.objc.borrow::<NSNumberFormatterHostObject>(this).padding_position
}
- (())setPaddingPosition:(NSUInteger)pos {
    env.objc.borrow_mut::<NSNumberFormatterHostObject>(this).padding_position = pos;
}

// =========================================================================
// MARK: - Leniency / validation
// =========================================================================

// `- (BOOL)isLenient`
// <https://developer.apple.com/documentation/foundation/nsnumberformatter/1413563-islenient>
- (bool)isLenient {
    env.objc.borrow::<NSNumberFormatterHostObject>(this).is_lenient
}
- (())setLenient:(bool)flag {
    env.objc.borrow_mut::<NSNumberFormatterHostObject>(this).is_lenient = flag;
}

// `- (BOOL)isPartialStringValidationEnabled`
// <https://developer.apple.com/documentation/foundation/nsnumberformatter/1416914-ispartialstringvalidationenabled>
- (bool)isPartialStringValidationEnabled {
    env.objc.borrow::<NSNumberFormatterHostObject>(this).partial_string_validation_enabled
}
- (())setPartialStringValidationEnabled:(bool)flag {
    env.objc.borrow_mut::<NSNumberFormatterHostObject>(this).partial_string_validation_enabled = flag;
}

@end

};
