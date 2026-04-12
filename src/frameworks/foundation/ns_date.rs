/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0.
 * If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//!
//! `NSDate`.

use super::ns_string::{from_rust_ordering, from_rust_string, to_rust_string};
use super::{NSComparisonResult, NSTimeInterval};
use crate::frameworks::core_foundation::time::{
    apple_epoch, CFAbsoluteTimeGetGregorianDate, SECS_FROM_UNIX_TO_APPLE_EPOCHS,
};
use crate::objc::{
    autorelease, id, msg, msg_class, nil, objc_classes, release, retain, ClassExports, HostObject,
    NSZonePtr,
};
use crate::frameworks::foundation::ns_keyed_unarchiver::decode_current_date;
use std::ops::{Add, Sub};
use std::time::{Duration, SystemTime};

// Time interval constants
const SECS_PER_DAY: NSTimeInterval = 86400.0;
const SECS_PER_HOUR: NSTimeInterval = 3600.0;
const SECS_PER_MINUTE: NSTimeInterval = 60.0;

#[derive(Default, Clone)]
pub(super) struct NSDateHostObject {
    pub(super) time_interval: NSTimeInterval,
}
impl HostObject for NSDateHostObject {}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation NSDate: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::<NSDateHostObject>::default();
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

// MARK: - Getting Current Date and Time

+ (NSTimeInterval)timeIntervalSinceReferenceDate {
    SystemTime::now()
        .duration_since(apple_epoch())
        .unwrap()
        .as_secs_f64()
}

+ (id)date {
    let new: id = msg![env; this alloc];
    let new: id = msg![env; new init];
    log_dbg!("[NSDate date] => {:?} ({:?}s)", new, env.objc.borrow::<NSDateHostObject>(new).time_interval);
    autorelease(env, new)
}

// MARK: - Creating Dates with Distant Past and Future

+ (id)distantFuture {
    // As of 2024, this approximately corresponds to 20 years into the future.
    // While `distantFuture` docs are talking in terms of centuries,
    // this should be OK to use for our purposes.
    let time_interval = SystemTime::now()
        .duration_since(apple_epoch())
        .unwrap()
        .as_secs_f64() * 2.0;
    let host_object = Box::new(NSDateHostObject {
        time_interval
    });
    let new = env.objc.alloc_object(this, host_object, &mut env.mem);

    log_dbg!("[(NSDate*){:?} distantFuture]: date {:?} (time_interval: {})", this, new, time_interval);
    autorelease(env, new)
}

+ (id)distantPast {
    // This corresponds to the Unix epoch from Apple's reference date.
    // While `distantPast` docs are talking in terms of centuries,
    // for our purposes it is OK to use the Unix epoch as a distant past.
    let time_interval = -(SECS_FROM_UNIX_TO_APPLE_EPOCHS as f64);
    let host_object = Box::new(NSDateHostObject {
        time_interval
    });
    let new = env.objc.alloc_object(this, host_object, &mut env.mem);

    log_dbg!("[(NSDate*){:?} distantPast]: date {:?} (time_interval: {})", this, new, time_interval);
    autorelease(env, new)
}

// MARK: - Creating Dates Relative to Other Dates

+ (id)dateWithTimeIntervalSinceNow:(NSTimeInterval)secs {
    let now: id = msg_class![env; NSDate date];
    msg![env; now addTimeInterval:secs]
}

+ (id)dateWithTimeIntervalSince1970:(NSTimeInterval)secs {
    let new: id = msg![env; this alloc];
    let new: id = msg![env; new initWithTimeIntervalSince1970:secs];
    autorelease(env, new)
}

+ (id)dateWithTimeInterval:(NSTimeInterval)secs
                 sinceDate:(id)date { // NSDate *
    let new: id = msg![env; this alloc];
    let new: id = msg![env; new initWithTimeInterval:secs sinceDate:date];
    autorelease(env, new)
}

+ (id)dateWithTimeIntervalSinceReferenceDate:(NSTimeInterval)secs {
    let new: id = msg![env; this alloc];
    let new: id = msg![env; new initWithTimeIntervalSinceReferenceDate:secs];
    autorelease(env, new)
}

// MARK: - Initializers

- (id)init {
    // "Date objects are immutable, representing an invariant time interval
    // relative to an absolute reference date (00:00:00 UTC on 1 January 2001)."
    let time_interval = SystemTime::now()
        .duration_since(apple_epoch())
        .unwrap()
        .as_secs_f64();
    env.objc.borrow_mut::<NSDateHostObject>(this).time_interval = time_interval;
    this
}

- (id)initWithTimeInterval:(NSTimeInterval)secs
                 sinceDate:(id)date { // NSDate *
    if date.is_null() {
        // If nil date provided, use reference date
        env.objc.borrow_mut::<NSDateHostObject>(this).time_interval = secs;
    } else {
        let time_interval = env.objc.borrow::<NSDateHostObject>(date).time_interval + secs;
        env.objc.borrow_mut::<NSDateHostObject>(this).time_interval = time_interval;
    }
    this
}

- (id)initWithTimeIntervalSinceNow:(NSTimeInterval)secs {
    let time_interval = SystemTime::now()
        .duration_since(apple_epoch())
        .unwrap()
        .as_secs_f64();
    env.objc.borrow_mut::<NSDateHostObject>(this).time_interval = time_interval + secs;
    this
}

- (id)initWithTimeIntervalSinceReferenceDate:(NSTimeInterval)secs {
    env.objc.borrow_mut::<NSDateHostObject>(this).time_interval = secs;
    this
}

- (id)initWithTimeIntervalSince1970:(NSTimeInterval)secs {
    let time_interval = -(SECS_FROM_UNIX_TO_APPLE_EPOCHS as f64) + secs;
    env.objc.borrow_mut::<NSDateHostObject>(this).time_interval = time_interval;
    this
}

// NSCoding implementation
- (id)initWithCoder:(id)coder {
    release(env, this);
    // Note: Assuming NSKeyedUnarchiver as coder here
    decode_current_date(env, coder)
}

// MARK: - Getting Time Intervals

- (NSTimeInterval)timeIntervalSinceDate:(id)anotherDate {
    if anotherDate.is_null() {
        // If nil, return interval since reference date
        return env.objc.borrow::<NSDateHostObject>(this).time_interval;
    }
    
    let host_object = env.objc.borrow::<NSDateHostObject>(this);
    let another_date_host_object = env.objc.borrow::<NSDateHostObject>(anotherDate);
    let result = host_object.time_interval - another_date_host_object.time_interval;
    log_dbg!("[(NSDate*){:?} ({:?}s) timeIntervalSinceDate:{:?} ({:?}s)] => {}", this, host_object.time_interval, anotherDate, another_date_host_object.time_interval, result);
    result
}

- (NSTimeInterval)timeIntervalSinceReferenceDate {
    env.objc.borrow::<NSDateHostObject>(this).time_interval
}

- (NSTimeInterval)timeIntervalSinceNow {
    let host_object = env.objc.borrow::<NSDateHostObject>(this);
    let time_interval = SystemTime::now()
        .duration_since(apple_epoch())
        .unwrap()
        .as_secs_f64();
    host_object.time_interval - time_interval
}

- (NSTimeInterval)timeIntervalSince1970 {
    let time_interval = env.objc.borrow::<NSDateHostObject>(this).time_interval;
    let new_time = if time_interval >= 0.0 {
        apple_epoch().add(Duration::from_secs_f64(time_interval))
    } else {
        apple_epoch().sub(Duration::from_secs_f64(-time_interval))
    };
    new_time
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs_f64()
}

// MARK: - Adding Time Intervals

- (id)addTimeInterval:(NSTimeInterval)seconds {
    let interval = env.objc.borrow::<NSDateHostObject>(this).time_interval + seconds;
    let date = msg_class![env; NSDate date];
    env.objc.borrow_mut::<NSDateHostObject>(date).time_interval = interval;
    date
}

- (id)dateByAddingTimeInterval:(NSTimeInterval)seconds {
    msg![env; this addTimeInterval:seconds]
}

// MARK: - Comparing Dates

- (NSComparisonResult)compare:(id)anotherDate { // NSDate *
    if anotherDate.is_null() {
        return 1;
        // NSOrderedDescending - this is later than nil
    }
    
    let host_object = env.objc.borrow::<NSDateHostObject>(this);
    let another_date_host_object = env.objc.borrow::<NSDateHostObject>(anotherDate);
    from_rust_ordering(host_object.time_interval.total_cmp(&another_date_host_object.time_interval))
}

- (bool)isEqualToDate:(id)anotherDate { // NSDate *
    if anotherDate.is_null() {
        return false;
    }
    
    if this == anotherDate {
        return true;
    }
    
    let host_object = env.objc.borrow::<NSDateHostObject>(this);
    let another_date_host_object = env.objc.borrow::<NSDateHostObject>(anotherDate);
    host_object.time_interval == another_date_host_object.time_interval
}

- (id)earlierDate:(id)anotherDate { // NSDate *
    if anotherDate.is_null() {
        return this;
    }
    
    let comparison: NSComparisonResult = msg![env; this compare:anotherDate];
    if comparison <= 0 {
        this
    } else {
        anotherDate
    }
}

- (id)laterDate:(id)anotherDate { // NSDate *
    if anotherDate.is_null() {
        return this;
    }
    
    let comparison: NSComparisonResult = msg![env; this compare:anotherDate];
    if comparison >= 0 {
        this
    } else {
        anotherDate
    }
}

// MARK: - Describing Dates

- (id)description {
    let time_interval = env.objc.borrow::<NSDateHostObject>(this).time_interval;
    let greg_date = CFAbsoluteTimeGetGregorianDate(env, time_interval, nil);
    // Format similar to NSDate description: "YYYY-MM-DD HH:MM:SS +0000"
    let year = greg_date.year;
    let month = greg_date.month;
    let day = greg_date.day;
    let hours = greg_date.hours;
    let minutes = greg_date.minutes;
    let seconds = greg_date.seconds;
    let secs_int = seconds as i32;
    // TODO: Use actual timezone instead of +0000
    let desc = format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02} +0000",
        year, month, day, hours, minutes, secs_int
    );
    let desc_string = from_rust_string(env, desc);
    autorelease(env, desc_string)
}

- (id)descriptionWithLocale:(id)locale { // NSLocale *
    // For now, ignore locale and use standard description
    let _locale = locale;
    // Suppress unused warning
    msg![env; this description]
}

// MARK: - NSCopying

- (id)copyWithZone:(NSZonePtr)_zone {
    // NSDate is immutable, so we can just retain
    retain(env, this)
}

// MARK: - NSObject overrides

- (bool)isEqual:(id)other {
    if other.is_null() {
        return false;
    }
    
    if this == other {
        return true;
    }
    
    // Check if other is an NSDate. Выносим вызов msg_class! в отдельную переменную,
    // чтобы избежать ошибки парсинга макроса msg!.
    let nsdate_class: id = msg_class![env; NSDate class];
    let is_kind_of_class: bool = msg![env; other isKindOfClass:nsdate_class];
    
    if !is_kind_of_class {
        return false;
    }
    
    msg![env; this isEqualToDate:other]
}

- (u32)hash {
    // Simple hash based on time interval
    let time_interval = env.objc.borrow::<NSDateHostObject>(this).time_interval;
    let bits = time_interval.to_bits();
    (bits ^ (bits >> 32)) as u32
}

@end

};

// MARK: - Additional helper functions for NSDate

/// Helper to get current date without autorelease
pub fn get_current_date(env: &mut crate::Environment) -> id {
    msg_class![env; NSDate date]
}

/// Helper to create date from Unix timestamp
pub fn date_from_unix_timestamp(env: &mut crate::Environment, timestamp: f64) -> id {
    msg_class![env; NSDate dateWithTimeIntervalSince1970:timestamp]
}

/// Helper to create date from time interval since reference date
pub fn date_from_reference_interval(env: &mut crate::Environment, interval: NSTimeInterval) -> id {
    msg_class![env; NSDate dateWithTimeIntervalSinceReferenceDate:interval]
}

/// Helper to get time interval from NSDate
pub fn get_time_interval(env: &mut crate::Environment, date: id) -> NSTimeInterval {
    if date.is_null() {
        return 0.0;
    }
    env.objc.borrow::<NSDateHostObject>(date).time_interval
}

/// Helper to check if a date is in the past
pub fn is_date_in_past(env: &mut crate::Environment, date: id) -> bool {
    if date.is_null() {
        return false;
    }
    let interval: NSTimeInterval = msg![env; date timeIntervalSinceNow];
    interval < 0.0
}

/// Helper to check if a date is in the future
pub fn is_date_in_future(env: &mut crate::Environment, date: id) -> bool {
    if date.is_null() {
        return false;
    }
    let interval: NSTimeInterval = msg![env; date timeIntervalSinceNow];
    interval > 0.0
}

/// Helper to get days between two dates
pub fn days_between_dates(env: &mut crate::Environment, from_date: id, to_date: id) -> i32 {
    if from_date.is_null() || to_date.is_null() {
        return 0;
    }
    let interval: NSTimeInterval = msg![env; to_date timeIntervalSinceDate:from_date];
    (interval / SECS_PER_DAY) as i32
}

/// Helper to get hours between two dates
pub fn hours_between_dates(env: &mut crate::Environment, from_date: id, to_date: id) -> i32 {
    if from_date.is_null() || to_date.is_null() {
        return 0;
    }
    let interval: NSTimeInterval = msg![env; to_date timeIntervalSinceDate:from_date];
    (interval / SECS_PER_HOUR) as i32
}

/// Helper to create a date by adding days to another date
pub fn date_by_adding_days(env: &mut crate::Environment, date: id, days: i32) -> id {
    if date.is_null() {
        return nil;
    }
    let interval = (days as f64) * SECS_PER_DAY;
    msg![env; date dateByAddingTimeInterval:interval]
}

/// Helper to create a date by adding hours to another date
pub fn date_by_adding_hours(env: &mut crate::Environment, date: id, hours: i32) -> id {
    if date.is_null() {
        return nil;
    }
    let interval = (hours as f64) * SECS_PER_HOUR;
    msg![env; date dateByAddingTimeInterval:interval]
}

/// Helper to create a date by adding minutes to another date
pub fn date_by_adding_minutes(env: &mut crate::Environment, date: id, minutes: i32) -> id {
    if date.is_null() {
        return nil;
    }
    let interval = (minutes as f64) * SECS_PER_MINUTE;
    msg![env; date dateByAddingTimeInterval:interval]
}

/// Helper to format date as ISO 8601 string
pub fn format_date_iso8601(env: &mut crate::Environment, date: id) -> String {
    if date.is_null() {
        return String::from("");
    }
    
    let time_interval = env.objc.borrow::<NSDateHostObject>(date).time_interval;
    let greg_date = CFAbsoluteTimeGetGregorianDate(env, time_interval, nil);
    // Copy packed struct fields to locals to avoid unaligned reference UB.
    let year = greg_date.year;
    let month = greg_date.month;
    let day = greg_date.day;
    let hours = greg_date.hours;
    let minutes = greg_date.minutes;
    let seconds = greg_date.seconds as i32;
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        year, month, day, hours, minutes, seconds
    )
}

/// Helper to get Gregorian components from date
pub fn get_date_components(env: &mut crate::Environment, date: id) -> (i32, i8, i8, i8, i8, f64) {
    if date.is_null() {
        return (0, 0, 0, 0, 0, 0.0);
    }
    
    let time_interval = env.objc.borrow::<NSDateHostObject>(date).time_interval;
    let greg_date = CFAbsoluteTimeGetGregorianDate(env, time_interval, nil);
    
    (
        greg_date.year,
        greg_date.month,
        greg_date.day,
        greg_date.hours,
        greg_date.minutes,
        greg_date.seconds,
    )
}

/// Helper to check if two dates are on the same day
pub fn are_dates_on_same_day(env: &mut crate::Environment, date1: id, date2: id) -> bool {
    if date1.is_null() || date2.is_null() {
        return false;
    }
    
    let (year1, month1, day1, _, _, _) = get_date_components(env, date1);
    let (year2, month2, day2, _, _, _) = get_date_components(env, date2);
    year1 == year2 && month1 == month2 && day1 == day2
}

/// Helper to get start of day for a date
pub fn start_of_day(env: &mut crate::Environment, date: id) -> id {
    if date.is_null() {
        return nil;
    }
    
    let time_interval = env.objc.borrow::<NSDateHostObject>(date).time_interval;
    let greg_date = CFAbsoluteTimeGetGregorianDate(env, time_interval, nil);
    // Calculate time interval for start of day (00:00:00)
    let day_start_interval = time_interval - 
        (greg_date.hours as f64 * SECS_PER_HOUR) -
        (greg_date.minutes as f64 * SECS_PER_MINUTE) -
        greg_date.seconds;
    msg_class![env; NSDate dateWithTimeIntervalSinceReferenceDate:day_start_interval]
}

/// Helper to get end of day for a date
pub fn end_of_day(env: &mut crate::Environment, date: id) -> id {
    if date.is_null() {
        return nil;
    }
    
    let start = start_of_day(env, date);
    let end_interval = SECS_PER_DAY - 0.001;
    // 23:59:59.999
    msg![env; start dateByAddingTimeInterval:end_interval]
}

