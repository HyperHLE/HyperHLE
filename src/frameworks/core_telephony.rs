/*
* This Source Code Form is subject to the terms of the Mozilla Public
* License, v. 2.0. If a copy of the MPL was not distributed with this
* file, You can obtain one at https://mozilla.org/MPL/2.0/.
*/
//! `CTTelephonyNetworkInfo` and related CoreTelephony stubs.
//!
//! touchHLE has no cellular modem. All properties return nil / empty values
//! so apps that probe for carrier info get a graceful "no service" result
//! rather than crashing.

use crate::frameworks::foundation::ns_string;
use crate::dyld::{ConstantExports, HostConstant};
use crate::objc::{
   id, msg, msg_class, nil, objc_classes, release, retain,
   ClassExports, HostObject, NSZonePtr,
};

// =========================================================================
// MARK: - CTCarrier host object
// =========================================================================

struct CTCarrierHostObject {
   /// NSString* — carrier name, e.g. "AT&T". nil = no service.
   carrier_name: id,
   /// NSString* — ISO country code, e.g. "us". nil = no service.
   iso_country_code: id,
   /// NSString* — mobile country code. nil = no service.
   mobile_country_code: id,
   /// NSString* — mobile network code. nil = no service.
   mobile_network_code: id,
   /// Whether VoIP is allowed over cellular.
   allows_voip: bool,
}
impl HostObject for CTCarrierHostObject {}

// =========================================================================
// MARK: - CTTelephonyNetworkInfo host object
// =========================================================================

struct CTTelephonyNetworkInfoHostObject {
   /// Weak delegate reference.
   delegate: id,
   /// The subscriber cellular provider (CTCarrier*).
   subscriber_cellular_provider: id,
   /// NSString* — current radio access technology constant.
   current_radio_access_technology: id,
}
impl HostObject for CTTelephonyNetworkInfoHostObject {}

// =========================================================================
// MARK: - Radio access technology constants
// =========================================================================

// These are the public CTRadioAccessTechnology* string constants.
// We export them as static &str so other modules can reference them.
pub const CTRadioAccessTechnologyGPRS:          &str = "CTRadioAccessTechnologyGPRS";
pub const CTRadioAccessTechnologyEdge:          &str = "CTRadioAccessTechnologyEdge";
pub const CTRadioAccessTechnologyWCDMA:         &str = "CTRadioAccessTechnologyWCDMA";
pub const CTRadioAccessTechnologyHSDPA:         &str = "CTRadioAccessTechnologyHSDPA";
pub const CTRadioAccessTechnologyHSUPA:         &str = "CTRadioAccessTechnologyHSUPA";
pub const CTRadioAccessTechnologyCDMA1x:        &str = "CTRadioAccessTechnologyCDMA1x";
pub const CTRadioAccessTechnologyCDMAEVDORev0:  &str = "CTRadioAccessTechnologyCDMAEVDORev0";
pub const CTRadioAccessTechnologyCDMAEVDORevA:  &str = "CTRadioAccessTechnologyCDMAEVDORevA";
pub const CTRadioAccessTechnologyCDMAEVDORevB:  &str = "CTRadioAccessTechnologyCDMAEVDORevB";
pub const CTRadioAccessTechnologyeHRPD:         &str = "CTRadioAccessTechnologyeHRPD";
pub const CTRadioAccessTechnologyLTE:           &str = "CTRadioAccessTechnologyLTE";

// =========================================================================
// MARK: - Notification name constants
// =========================================================================

pub const CTRadioAccessTechnologyDidChangeNotification: &str =
   "CTRadioAccessTechnologyDidChangeNotification";
pub const CTServiceRadioAccessTechnologyDidChangeNotification: &str =
   "CTServiceRadioAccessTechnologyDidChangeNotification";

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

// =========================================================================
// MARK: - CTCarrier
// =========================================================================

@implementation CTCarrier: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
   let host_object = Box::new(CTCarrierHostObject {
       carrier_name:        nil,
       iso_country_code:    nil,
       mobile_country_code: nil,
       mobile_network_code: nil,
       allows_voip:         false,
   });
   env.objc.alloc_object(this, host_object, &mut env.mem)
}

- (id)init {
   this
}

- (())dealloc {
   let h = env.objc.borrow::<CTCarrierHostObject>(this);
   let (name, iso, mcc, mnc) = (
       h.carrier_name,
       h.iso_country_code,
       h.mobile_country_code,
       h.mobile_network_code,
   );
   release(env, name);
   release(env, iso);
   release(env, mcc);
   release(env, mnc);
   env.objc.dealloc_object(this, &mut env.mem)
}

// MARK: Properties

// The name of the user's cellular service provider.
// Returns nil when no service is available.
- (id)carrierName { // NSString*
   env.objc.borrow::<CTCarrierHostObject>(this).carrier_name
}

// The ISO country code for the user's cellular service provider.
- (id)isoCountryCode { // NSString*
   env.objc.borrow::<CTCarrierHostObject>(this).iso_country_code
}

// The mobile country code for the user's cellular service provider.
- (id)mobileCountryCode { // NSString*
   env.objc.borrow::<CTCarrierHostObject>(this).mobile_country_code
}

// The mobile network code for the user's cellular service provider.
- (id)mobileNetworkCode { // NSString*
   env.objc.borrow::<CTCarrierHostObject>(this).mobile_network_code
}

// Whether the carrier allows VoIP calls on its network.
- (bool)allowsVOIP {
   env.objc.borrow::<CTCarrierHostObject>(this).allows_voip
}

// MARK: NSCopying

- (id)copyWithZone:(NSZonePtr)_zone {
   retain(env, this);
   this
}

// MARK: Description

- (id)description {
   let name = env.objc.borrow::<CTCarrierHostObject>(this).carrier_name;
   let name_str = if name != nil {
       ns_string::to_rust_string(env, name).into_owned()
   } else {
       "(no service)".into()
   };
   let s = format!("<CTCarrier: {:?}; carrier={}>", this, name_str);
   let cstr = env.mem.alloc_and_write_cstr(s.as_bytes());
   msg_class![env; NSString stringWithUTF8String:cstr]
}

@end

// =========================================================================
// MARK: - CTTelephonyNetworkInfo
// =========================================================================

@implementation CTTelephonyNetworkInfo: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
   let host_object = Box::new(CTTelephonyNetworkInfoHostObject {
       delegate:                       nil,
       subscriber_cellular_provider:   nil,
       current_radio_access_technology: nil,
   });
   env.objc.alloc_object(this, host_object, &mut env.mem)
}

- (id)init {
   // Create a stub CTCarrier representing "no service".
   let carrier: id = msg_class![env; CTCarrier alloc];
   let carrier: id = msg![env; carrier init];
   // Do NOT retain here — we'll store it retained below.
   retain(env, carrier);
   env.objc.borrow_mut::<CTTelephonyNetworkInfoHostObject>(this)
       .subscriber_cellular_provider = carrier;
   this
}

- (())dealloc {
   let h = env.objc.borrow::<CTTelephonyNetworkInfoHostObject>(this);
   let (delegate, provider, rat) = (
       h.delegate,
       h.subscriber_cellular_provider,
       h.current_radio_access_technology,
   );
   // delegate is weak — do not release.
   let _ = delegate;
   release(env, provider);
   release(env, rat);
   env.objc.dealloc_object(this, &mut env.mem)
}

// MARK: Delegate

// The delegate is a weak reference (not retained) per Apple's docs.
- (id)delegate {
   env.objc.borrow::<CTTelephonyNetworkInfoHostObject>(this).delegate
}

- (())setDelegate:(id)delegate {
   // Weak — no retain/release.
   env.objc.borrow_mut::<CTTelephonyNetworkInfoHostObject>(this).delegate = delegate;
}

// MARK: Cellular provider

// Returns a `CTCarrier` object describing the subscriber's cellular provider.
// In touchHLE this is always a stub carrier with all-nil properties,
// representing "no service".
- (id)subscriberCellularProvider { // CTCarrier*
   env.objc.borrow::<CTTelephonyNetworkInfoHostObject>(this)
       .subscriber_cellular_provider
}

// iOS 12+ variant that returns a dictionary keyed by service identifier.
// We return a single-entry dictionary with a dummy service ID.
- (id)serviceSubscriberCellularProviders { // NSDictionary<NSString*, CTCarrier*>*
   let provider = env.objc.borrow::<CTTelephonyNetworkInfoHostObject>(this)
       .subscriber_cellular_provider;
   let dict: id = msg_class![env; NSMutableDictionary new];
   let key = ns_string::get_static_str(env, "0000000100000001");
   if provider != nil {
       let _: () = msg![env; dict setObject:provider forKey:key];
   }
   crate::objc::autorelease(env, dict)
}

// MARK: Radio access technology

// The current radio access technology, or nil if not connected.
// touchHLE reports nil (no cellular connection).
- (id)currentRadioAccessTechnology { // NSString*
   env.objc.borrow::<CTTelephonyNetworkInfoHostObject>(this)
       .current_radio_access_technology
}

// iOS 12+ dictionary variant.
- (id)serviceCurrentRadioAccessTechnology { // NSDictionary<NSString*, NSString*>*
   // Return an empty dictionary — no cellular service.
   let dict: id = msg_class![env; NSDictionary dictionary];
   crate::objc::autorelease(env, dict)
}

// MARK: Description

- (id)description {
   let rat = env.objc.borrow::<CTTelephonyNetworkInfoHostObject>(this)
       .current_radio_access_technology;
   let rat_str = if rat != nil {
       ns_string::to_rust_string(env, rat).into_owned()
   } else {
       "(none)".into()
   };
   let s = format!(
       "<CTTelephonyNetworkInfo: {:?}; radioAccessTechnology={}>",
       this, rat_str
   );
   let cstr = env.mem.alloc_and_write_cstr(s.as_bytes());
   msg_class![env; NSString stringWithUTF8String:cstr]
}

@end

};

// =========================================================================
// MARK: - Constant exports
// =========================================================================


pub const CONSTANTS: ConstantExports = &[
   // Radio access technology strings
   ("_CTRadioAccessTechnologyGPRS",
       HostConstant::NSString(CTRadioAccessTechnologyGPRS)),
   ("_CTRadioAccessTechnologyEdge",
       HostConstant::NSString(CTRadioAccessTechnologyEdge)),
   ("_CTRadioAccessTechnologyWCDMA",
       HostConstant::NSString(CTRadioAccessTechnologyWCDMA)),
   ("_CTRadioAccessTechnologyHSDPA",
       HostConstant::NSString(CTRadioAccessTechnologyHSDPA)),
   ("_CTRadioAccessTechnologyHSUPA",
       HostConstant::NSString(CTRadioAccessTechnologyHSUPA)),
   ("_CTRadioAccessTechnologyCDMA1x",
       HostConstant::NSString(CTRadioAccessTechnologyCDMA1x)),
   ("_CTRadioAccessTechnologyCDMAEVDORev0",
       HostConstant::NSString(CTRadioAccessTechnologyCDMAEVDORev0)),
   ("_CTRadioAccessTechnologyCDMAEVDORevA",
       HostConstant::NSString(CTRadioAccessTechnologyCDMAEVDORevA)),
   ("_CTRadioAccessTechnologyCDMAEVDORevB",
       HostConstant::NSString(CTRadioAccessTechnologyCDMAEVDORevB)),
   ("_CTRadioAccessTechnologyeHRPD",
       HostConstant::NSString(CTRadioAccessTechnologyeHRPD)),
   ("_CTRadioAccessTechnologyLTE",
       HostConstant::NSString(CTRadioAccessTechnologyLTE)),
   // Notification names
   ("_CTRadioAccessTechnologyDidChangeNotification",
       HostConstant::NSString(CTRadioAccessTechnologyDidChangeNotification)),
   ("_CTServiceRadioAccessTechnologyDidChangeNotification",
       HostConstant::NSString(CTServiceRadioAccessTechnologyDidChangeNotification)),
];
