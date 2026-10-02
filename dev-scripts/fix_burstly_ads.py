#!/usr/bin/env python3
"""Minimal Burstly/AdColony fake-class + toJSONAs compatibility."""
from pathlib import Path

def patch_classes():
    p = Path("src/objc/classes.rs")
    t = p.read_text()
    if 'name.starts_with("Burstly")' in t and t.count("Burstly") >= 2:
        print("classes.rs already patched")
        return
    needle = '        || name.starts_with("ALPrivacy") // AppLovin privacy settings\n    )'
    insert = '''        || name.starts_with("ALPrivacy") // AppLovin privacy settings
        // Burstly / AdColony / Chartboost: old ad SDKs (e.g. My Little Pony).
        // Without fakes, network stubs leave half-init objects \u2192 NULL-PAGE / nil isa.
        || name.starts_with("Burstly")
        || name.starts_with("AdColony")
        || name.starts_with("Chartboost")
    )'''
    if needle not in t:
        raise SystemExit("site1 needle missing")
    t = t.replace(needle, insert, 1)
    needle2 = '''            let is_fake = name.starts_with("AdMob")
                || name.starts_with("AltAds")
                || name.starts_with("Mobclix")
                || name.starts_with("FB")
                || name.starts_with("Flurry")
                || name.starts_with("OpenFeint")
                || name.starts_with("Tapjoy")
                || name.starts_with("UA")
                || name.starts_with("GAD")
                || name.starts_with("iSimulate")
                // SpringBoard private classes'''
    insert2 = '''            let is_fake = name.starts_with("AdMob")
                || name.starts_with("AltAds")
                || name.starts_with("Mobclix")
                || name.starts_with("FB")
                || name.starts_with("Flurry")
                || name.starts_with("OpenFeint")
                || name.starts_with("Tapjoy")
                || name.starts_with("UA")
                || name.starts_with("GAD")
                || name.starts_with("iSimulate")
                || name.starts_with("Burstly")
                || name.starts_with("AdColony")
                || name.starts_with("Chartboost")
                || name.starts_with("RevMob")
                // SpringBoard private classes'''
    if needle2 not in t:
        raise SystemExit("site2 needle missing")
    t = t.replace(needle2, insert2, 1)
    p.write_text(t)
    print("classes.rs patched")

def patch_ns_object():
    p = Path("src/frameworks/foundation/ns_object.rs")
    t = p.read_text()
    if "toJSONAs:" in t:
        print("ns_object.rs already patched")
        return
    needle = """- (id)debugDescription {
    msg![env; this description]
}"""
    insert = """- (id)debugDescription {
    msg![env; this description]
}

// TouchJSON / SBJSON-style selectors used by Burstly / AdColony response
// serializers. Returning a real empty JSON string avoids the guest treating
// an unrecognized-selector fallback as a live object pointer (e.g. 0x1) and
// walking NULL-PAGE memory.
- (id)toJSONAs:(bool)_as
    excludingInArray:(id)_excluding
    withTranslations:(id)_translations {
    let dict_class = env.objc.get_known_class("NSDictionary", &mut env.mem);
    let array_class = env.objc.get_known_class("NSArray", &mut env.mem);
    let is_dict: bool = msg![env; this isKindOfClass:dict_class];
    let is_array: bool = msg![env; this isKindOfClass:array_class];
    if is_dict || is_array {
        let data: id = msg_class![env; NSJSONSerialization dataWithJSONObject:this
                                                                      options:0u32
                                                                        error:(crate::mem::MutPtr::null())];
        if data != nil {
            let s: id = msg_class![env; NSString alloc];
            let s: id = msg![env; s initWithData:data encoding:4u32]; // NSUTF8StringEncoding
            if s != nil {
                return autorelease(env, s);
            }
        }
    }
    let empty = from_rust_string(env, "{}".to_string());
    autorelease(env, empty)
}

- (id)JSONRepresentation {
    msg![env; this toJSONAs:false excludingInArray:nil withTranslations:nil]
}"""
    if needle not in t:
        raise SystemExit("ns_object needle missing")
    t = t.replace(needle, insert, 1)
    p.write_text(t)
    print("ns_object.rs patched")

if __name__ == "__main__":
    patch_classes()
    patch_ns_object()
    print("DONE")
