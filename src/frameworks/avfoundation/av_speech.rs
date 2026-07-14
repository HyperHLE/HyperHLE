/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `AVSpeechSynthesizer`, `AVSpeechUtterance` and `AVSpeechSynthesisVoice`.
//!
//! Text-to-speech from the AVFoundation framework (iOS 7.0+). touchHLE has no
//! host text-to-speech engine, so no audio is actually rendered, but the full
//! object model, the utterance queue state machine, and the delegate callbacks
//! are implemented faithfully so that guest apps observe the documented
//! behaviour (`isSpeaking` transitions, `speechSynthesizer:didStart…` /
//! `didFinish…` / `didPause…` / `didContinue…` / `didCancel…` etc.).
//!
//! References:
//! - <https://developer.apple.com/documentation/avfaudio/avspeechsynthesizer>
//! - <https://developer.apple.com/documentation/avfaudio/avspeechutterance>
//! - <https://developer.apple.com/documentation/avfaudio/avspeechsynthesisvoice>
//! - <https://developer.apple.com/documentation/avfaudio/avspeechsynthesizerdelegate>

use crate::frameworks::foundation::{ns_string, NSInteger, NSRange, NSTimeInterval, NSUInteger};
use crate::objc::{
    autorelease, id, msg, msg_class, nil, objc_classes, release, retain, ClassExports, HostObject,
    NSZonePtr, SEL,
};
use crate::Environment;

/// `AVSpeechBoundary` — specifies when to pause or stop speech.
/// <https://developer.apple.com/documentation/avfaudio/avspeechboundary>
type AVSpeechBoundary = NSInteger;
#[allow(dead_code)]
const AVSpeechBoundaryImmediate: AVSpeechBoundary = 0;
#[allow(dead_code)]
const AVSpeechBoundaryWord: AVSpeechBoundary = 1;

/// Apple documents these as opaque `float` constants. The concrete values are
/// only observable through arithmetic the app performs against them, so the
/// canonical iOS values (min 0.0, max 1.0, default 0.5) are sufficient.
const AV_SPEECH_UTTERANCE_MINIMUM_SPEECH_RATE: f32 = 0.0;
const AV_SPEECH_UTTERANCE_MAXIMUM_SPEECH_RATE: f32 = 1.0;
const AV_SPEECH_UTTERANCE_DEFAULT_SPEECH_RATE: f32 = 0.5;

#[derive(Default)]
struct AVSpeechUtteranceHostObject {
    /// Retained `NSString` with the text to speak.
    speech_string: id,
    /// Retained `AVSpeechSynthesisVoice`, or `nil` for the system default.
    voice: id,
    rate: f32,
    pitch_multiplier: f32,
    volume: f32,
    pre_utterance_delay: NSTimeInterval,
    post_utterance_delay: NSTimeInterval,
}
impl HostObject for AVSpeechUtteranceHostObject {}

#[derive(Default)]
struct AVSpeechSynthesisVoiceHostObject {
    /// Retained `NSString` BCP-47 language code, e.g. `"en-US"`.
    language: id,
}
impl HostObject for AVSpeechSynthesisVoiceHostObject {}

#[derive(Default)]
struct AVSpeechSynthesizerHostObject {
    /// Weak reference to the delegate (delegates are not retained, matching
    /// Cocoa semantics).
    delegate: id,
    /// FIFO queue of retained `AVSpeechUtterance` objects still to be spoken.
    queue: Vec<id>,
    speaking: bool,
    paused: bool,
}
impl HostObject for AVSpeechSynthesizerHostObject {}

/// Register a delegate selector and return whether the delegate implements it.
/// Registering first ensures `respondsToSelector:`/`msg!` never panic with
/// "unknown selector" when nothing in the guest binary referenced the method.
fn delegate_responds(env: &mut Environment, delegate: id, name: &str) -> bool {
    if delegate == nil {
        return false;
    }
    let sel: SEL = env
        .objc
        .register_host_selector(name.to_string(), &mut env.mem);
    msg![env; delegate respondsToSelector:sel]
}

/// Speak the utterances in the queue synchronously: for each one fire
/// `didStart`, an optional whole-string `willSpeakRange` progress callback,
/// then (since we have no audio engine) `didFinish`, popping the utterance and
/// advancing. This preserves the documented ordering and `isSpeaking`/queue
/// semantics.
fn pump_queue(env: &mut Environment, this: id) {
    loop {
        let (delegate, next) = {
            let host = env.objc.borrow::<AVSpeechSynthesizerHostObject>(this);
            (host.delegate, host.queue.first().copied())
        };
        let Some(utterance) = next else {
            env.objc
                .borrow_mut::<AVSpeechSynthesizerHostObject>(this)
                .speaking = false;
            return;
        };

        env.objc
            .borrow_mut::<AVSpeechSynthesizerHostObject>(this)
            .speaking = true;

        if delegate_responds(env, delegate, "speechSynthesizer:didStartSpeechUtterance:") {
            () = msg![env; delegate speechSynthesizer:this didStartSpeechUtterance:utterance];
        }

        if delegate_responds(
            env,
            delegate,
            "speechSynthesizer:willSpeakRangeOfSpeechString:utterance:",
        ) {
            let speech_string: id = msg![env; utterance speechString];
            let length: NSUInteger = if speech_string == nil {
                0
            } else {
                msg![env; speech_string length]
            };
            let range = NSRange {
                location: 0,
                length,
            };
            () = msg![env; delegate speechSynthesizer:this
                                    willSpeakRangeOfSpeechString:range
                                    utterance:utterance];
        }

        if delegate_responds(env, delegate, "speechSynthesizer:didFinishSpeechUtterance:") {
            () = msg![env; delegate speechSynthesizer:this didFinishSpeechUtterance:utterance];
        }

        // Pop and release the finished utterance, then continue with the next.
        // (Re-check the queue: a delegate callback may have paused, stopped, or
        // enqueued more speech.)
        let popped = {
            let host = env.objc.borrow_mut::<AVSpeechSynthesizerHostObject>(this);
            if host.queue.first().copied() == Some(utterance) {
                Some(host.queue.remove(0))
            } else {
                None
            }
        };
        if let Some(finished) = popped {
            release(env, finished);
        }

        // If a delegate paused us, stop draining until continueSpeaking.
        if env
            .objc
            .borrow::<AVSpeechSynthesizerHostObject>(this)
            .paused
        {
            return;
        }
    }
}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

// MARK: - AVSpeechSynthesisVoice

@implementation AVSpeechSynthesisVoice: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::<AVSpeechSynthesisVoiceHostObject>::default();
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

// <https://developer.apple.com/documentation/avfaudio/avspeechsynthesisvoice/voicewithlanguage:>
+ (id)voiceWithLanguage:(id)language { // NSString*
    let voice: id = msg![env; this alloc];
    let language = if language == nil {
        msg_class![env; AVSpeechSynthesisVoice currentLanguageCode]
    } else {
        language
    };
    retain(env, language);
    env.objc.borrow_mut::<AVSpeechSynthesisVoiceHostObject>(voice).language = language;
    autorelease(env, voice)
}

// <https://developer.apple.com/documentation/avfaudio/avspeechsynthesisvoice/currentlanguagecode>
+ (id)currentLanguageCode { // NSString*
    // touchHLE reports a single stable locale; en-US is a safe default that
    // every iOS voice table contains.
    ns_string::get_static_str(env, "en-US")
}

+ (id)speechVoices { // NSArray*
    let voice: id = msg_class![env; AVSpeechSynthesisVoice voiceWithLanguage:nil];
    msg_class![env; NSArray arrayWithObject:voice]
}

- (id)language { // NSString*
    env.objc.borrow::<AVSpeechSynthesisVoiceHostObject>(this).language
}

- (id)name { // NSString*
    ns_string::get_static_str(env, "touchHLE")
}

- (())dealloc {
    let language = env.objc.borrow::<AVSpeechSynthesisVoiceHostObject>(this).language;
    release(env, language);
    env.objc.dealloc_object(this, &mut env.mem)
}

@end

// MARK: - AVSpeechUtterance

@implementation AVSpeechUtterance: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(AVSpeechUtteranceHostObject {
        speech_string: nil,
        voice: nil,
        rate: AV_SPEECH_UTTERANCE_DEFAULT_SPEECH_RATE,
        pitch_multiplier: 1.0,
        volume: 1.0,
        pre_utterance_delay: 0.0,
        post_utterance_delay: 0.0,
    });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

// <https://developer.apple.com/documentation/avfaudio/avspeechutterance/speechutterancewithstring:>
+ (id)speechUtteranceWithString:(id)string { // NSString*
    let utterance: id = msg![env; this alloc];
    let utterance: id = msg![env; utterance initWithString:string];
    autorelease(env, utterance)
}

// <https://developer.apple.com/documentation/avfaudio/avspeechutterance/init(string:)>
- (id)initWithString:(id)string { // NSString*
    retain(env, string);
    env.objc.borrow_mut::<AVSpeechUtteranceHostObject>(this).speech_string = string;
    this
}

- (id)speechString { // NSString*
    env.objc.borrow::<AVSpeechUtteranceHostObject>(this).speech_string
}

- (id)voice { // AVSpeechSynthesisVoice*
    env.objc.borrow::<AVSpeechUtteranceHostObject>(this).voice
}
- (())setVoice:(id)voice { // AVSpeechSynthesisVoice*
    let old = env.objc.borrow::<AVSpeechUtteranceHostObject>(this).voice;
    retain(env, voice);
    release(env, old);
    env.objc.borrow_mut::<AVSpeechUtteranceHostObject>(this).voice = voice;
}

- (f32)rate {
    env.objc.borrow::<AVSpeechUtteranceHostObject>(this).rate
}
- (())setRate:(f32)rate {
    let rate = rate.clamp(
        AV_SPEECH_UTTERANCE_MINIMUM_SPEECH_RATE,
        AV_SPEECH_UTTERANCE_MAXIMUM_SPEECH_RATE,
    );
    env.objc.borrow_mut::<AVSpeechUtteranceHostObject>(this).rate = rate;
}

- (f32)pitchMultiplier {
    env.objc.borrow::<AVSpeechUtteranceHostObject>(this).pitch_multiplier
}
- (())setPitchMultiplier:(f32)pitch {
    // Apple documents the valid range as [0.5, 2.0].
    let pitch = pitch.clamp(0.5, 2.0);
    env.objc.borrow_mut::<AVSpeechUtteranceHostObject>(this).pitch_multiplier = pitch;
}

- (f32)volume {
    env.objc.borrow::<AVSpeechUtteranceHostObject>(this).volume
}
- (())setVolume:(f32)volume {
    let volume = volume.clamp(0.0, 1.0);
    env.objc.borrow_mut::<AVSpeechUtteranceHostObject>(this).volume = volume;
}

- (NSTimeInterval)preUtteranceDelay {
    env.objc.borrow::<AVSpeechUtteranceHostObject>(this).pre_utterance_delay
}
- (())setPreUtteranceDelay:(NSTimeInterval)delay {
    env.objc.borrow_mut::<AVSpeechUtteranceHostObject>(this).pre_utterance_delay = delay;
}

- (NSTimeInterval)postUtteranceDelay {
    env.objc.borrow::<AVSpeechUtteranceHostObject>(this).post_utterance_delay
}
- (())setPostUtteranceDelay:(NSTimeInterval)delay {
    env.objc.borrow_mut::<AVSpeechUtteranceHostObject>(this).post_utterance_delay = delay;
}

- (())dealloc {
    let &AVSpeechUtteranceHostObject { speech_string, voice, .. } =
        env.objc.borrow(this);
    release(env, speech_string);
    release(env, voice);
    env.objc.dealloc_object(this, &mut env.mem)
}

@end

// MARK: - AVSpeechSynthesizer

@implementation AVSpeechSynthesizer: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::<AVSpeechSynthesizerHostObject>::default();
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

- (id)init {
    this
}

- (id)delegate {
    env.objc.borrow::<AVSpeechSynthesizerHostObject>(this).delegate
}
- (())setDelegate:(id)delegate {
    env.objc.borrow_mut::<AVSpeechSynthesizerHostObject>(this).delegate = delegate;
}

- (bool)isSpeaking {
    let host = env.objc.borrow::<AVSpeechSynthesizerHostObject>(this);
    // Per Apple: true while speaking OR paused with utterances still queued.
    host.speaking || (host.paused && !host.queue.is_empty())
}

- (bool)isPaused {
    env.objc.borrow::<AVSpeechSynthesizerHostObject>(this).paused
}

// <https://developer.apple.com/documentation/avfaudio/avspeechsynthesizer/speak(_:)>
// (Objective-C selector: `speakUtterance:`.)
- (())speakUtterance:(id)utterance { // AVSpeechUtterance*
    if utterance == nil {
        return;
    }
    log_dbg!("[(AVSpeechSynthesizer*){:?} speakUtterance:{:?}]", this, utterance);
    retain(env, utterance);
    env.objc
        .borrow_mut::<AVSpeechSynthesizerHostObject>(this)
        .queue
        .push(utterance);
    // If we're idle, begin draining the queue now. If we're already speaking
    // (a re-entrant call from a delegate) or paused, the utterance simply
    // waits its turn.
    let (speaking, paused) = {
        let host = env.objc.borrow::<AVSpeechSynthesizerHostObject>(this);
        (host.speaking, host.paused)
    };
    if !speaking && !paused {
        pump_queue(env, this);
    }
}

// <https://developer.apple.com/documentation/avfaudio/avspeechsynthesizer/pausespeaking(at:)>
- (bool)pauseSpeakingAtBoundary:(AVSpeechBoundary)_boundary {
    let (was_speaking, delegate, current) = {
        let host = env.objc.borrow::<AVSpeechSynthesizerHostObject>(this);
        (host.speaking, host.delegate, host.queue.first().copied())
    };
    if !was_speaking {
        return false;
    }
    {
        let host = env.objc.borrow_mut::<AVSpeechSynthesizerHostObject>(this);
        host.paused = true;
        host.speaking = false;
    }
    if let Some(utterance) = current {
        if delegate_responds(env, delegate, "speechSynthesizer:didPauseSpeechUtterance:") {
            () = msg![env; delegate speechSynthesizer:this didPauseSpeechUtterance:utterance];
        }
    }
    true
}

// <https://developer.apple.com/documentation/avfaudio/avspeechsynthesizer/continuespeaking()>
- (bool)continueSpeaking {
    let (was_paused, delegate, current) = {
        let host = env.objc.borrow::<AVSpeechSynthesizerHostObject>(this);
        (host.paused, host.delegate, host.queue.first().copied())
    };
    if !was_paused {
        return false;
    }
    {
        let host = env.objc.borrow_mut::<AVSpeechSynthesizerHostObject>(this);
        host.paused = false;
        host.speaking = true;
    }
    if let Some(utterance) = current {
        if delegate_responds(env, delegate, "speechSynthesizer:didContinueSpeechUtterance:") {
            () = msg![env; delegate speechSynthesizer:this didContinueSpeechUtterance:utterance];
        }
    }
    // Resume draining the queue.
    pump_queue(env, this);
    true
}

// <https://developer.apple.com/documentation/avfaudio/avspeechsynthesizer/stopspeaking(at:)>
- (bool)stopSpeakingAtBoundary:(AVSpeechBoundary)_boundary {
    let (was_active, delegate, current, remaining) = {
        let host = env.objc.borrow::<AVSpeechSynthesizerHostObject>(this);
        (
            host.speaking || host.paused,
            host.delegate,
            host.queue.first().copied(),
            host.queue.clone(),
        )
    };
    if !was_active {
        return false;
    }
    {
        let host = env.objc.borrow_mut::<AVSpeechSynthesizerHostObject>(this);
        host.speaking = false;
        host.paused = false;
        host.queue.clear();
    }
    // The currently-spoken utterance is reported as cancelled.
    if let Some(utterance) = current {
        if delegate_responds(env, delegate, "speechSynthesizer:didCancelSpeechUtterance:") {
            () = msg![env; delegate speechSynthesizer:this didCancelSpeechUtterance:utterance];
        }
    }
    // Balance the retains taken in speakUtterance:.
    for utterance in remaining {
        release(env, utterance);
    }
    true
}

- (())dealloc {
    let remaining = std::mem::take(
        &mut env.objc.borrow_mut::<AVSpeechSynthesizerHostObject>(this).queue,
    );
    for utterance in remaining {
        release(env, utterance);
    }
    env.objc.dealloc_object(this, &mut env.mem)
}

@end

};
