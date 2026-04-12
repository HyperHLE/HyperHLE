/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `MFMailComposeViewController` and `MFMailComposeViewControllerDelegate`.

use crate::frameworks::foundation::ns_string;
use crate::objc::{
    id, msg, msg_class, nil, objc_classes, release, retain, ClassExports, HostObject, NSZonePtr,
};

// MARK: - MFMailComposeResult constants

pub type MFMailComposeResult = i32;
pub const MF_MAIL_COMPOSE_RESULT_CANCELLED: MFMailComposeResult = 0;
pub const MF_MAIL_COMPOSE_RESULT_SAVED:     MFMailComposeResult = 1;
pub const MF_MAIL_COMPOSE_RESULT_SENT:      MFMailComposeResult = 2;
pub const MF_MAIL_COMPOSE_RESULT_FAILED:    MFMailComposeResult = 3;

struct MFMailComposeViewControllerHostObject {
    /// Delegate id (weak reference per Apple convention)
    mail_compose_delegate: id,
    /// NSString* subject line
    subject: id,
    /// NSArray* of NSString* — To recipients
    to_recipients: id,
    /// NSArray* of NSString* — CC recipients
    cc_recipients: id,
    /// NSArray* of NSString* — BCC recipients
    bcc_recipients: id,
    /// NSString* message body
    body: id,
    /// Whether the body is HTML
    body_is_html: bool,
}
impl HostObject for MFMailComposeViewControllerHostObject {}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation MFMailComposeViewController: UINavigationController

// MARK: - Class-level availability check

+ (bool)canSendMail {
    // touchHLE has no mail infrastructure — always report unavailable.
    // Apps are expected to check this before presenting the composer.
    log!("MFMailComposeViewController: canSendMail — returning NO (no mail support)");
    false
}

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(MFMailComposeViewControllerHostObject {
        mail_compose_delegate: nil,
        subject: nil,
        to_recipients: nil,
        cc_recipients: nil,
        bcc_recipients: nil,
        body: nil,
        body_is_html: false,
    });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

- (id)init {
    this
}

- (())dealloc {
    let host = env.objc.borrow::<MFMailComposeViewControllerHostObject>(this);
    let (delegate, subject, to, cc, bcc, body) = (
        host.mail_compose_delegate,
        host.subject,
        host.to_recipients,
        host.cc_recipients,
        host.bcc_recipients,
        host.body,
    );
    release(env, delegate);
    release(env, subject);
    release(env, to);
    release(env, cc);
    release(env, bcc);
    release(env, body);
    env.objc.dealloc_object(this, &mut env.mem)
}

// MARK: - Delegate

- (id)mailComposeDelegate {
    env.objc.borrow::<MFMailComposeViewControllerHostObject>(this).mail_compose_delegate
}

- (())setMailComposeDelegate:(id)delegate {
    let old = env.objc.borrow::<MFMailComposeViewControllerHostObject>(this).mail_compose_delegate;
    release(env, old);
    retain(env, delegate);
    env.objc.borrow_mut::<MFMailComposeViewControllerHostObject>(this)
        .mail_compose_delegate = delegate;
}

// MARK: - Subject

- (())setSubject:(id)subject { // NSString*
    let old = env.objc.borrow::<MFMailComposeViewControllerHostObject>(this).subject;
    release(env, old);
    retain(env, subject);
    env.objc.borrow_mut::<MFMailComposeViewControllerHostObject>(this).subject = subject;
}

- (id)subject { // NSString*
    env.objc.borrow::<MFMailComposeViewControllerHostObject>(this).subject
}

// MARK: - Recipients

- (())setToRecipients:(id)recipients { // NSArray<NSString*>*
    let old = env.objc.borrow::<MFMailComposeViewControllerHostObject>(this).to_recipients;
    release(env, old);
    retain(env, recipients);
    env.objc.borrow_mut::<MFMailComposeViewControllerHostObject>(this).to_recipients = recipients;
}

- (id)toRecipients { // NSArray<NSString*>*
    env.objc.borrow::<MFMailComposeViewControllerHostObject>(this).to_recipients
}

- (())setCcRecipients:(id)recipients { // NSArray<NSString*>*
    let old = env.objc.borrow::<MFMailComposeViewControllerHostObject>(this).cc_recipients;
    release(env, old);
    retain(env, recipients);
    env.objc.borrow_mut::<MFMailComposeViewControllerHostObject>(this).cc_recipients = recipients;
}

- (id)ccRecipients { // NSArray<NSString*>*
    env.objc.borrow::<MFMailComposeViewControllerHostObject>(this).cc_recipients
}

- (())setBccRecipients:(id)recipients { // NSArray<NSString*>*
    let old = env.objc.borrow::<MFMailComposeViewControllerHostObject>(this).bcc_recipients;
    release(env, old);
    retain(env, recipients);
    env.objc.borrow_mut::<MFMailComposeViewControllerHostObject>(this).bcc_recipients = recipients;
}

- (id)bccRecipients { // NSArray<NSString*>*
    env.objc.borrow::<MFMailComposeViewControllerHostObject>(this).bcc_recipients
}

// MARK: - Body

- (())setMessageBody:(id)body // NSString*
             isHTML:(bool)is_html {
    let old = env.objc.borrow::<MFMailComposeViewControllerHostObject>(this).body;
    release(env, old);
    retain(env, body);
    {
        let host = env.objc.borrow_mut::<MFMailComposeViewControllerHostObject>(this);
        host.body = body;
        host.body_is_html = is_html;
    }
}

- (id)messageBody { // NSString*
    env.objc.borrow::<MFMailComposeViewControllerHostObject>(this).body
}

- (bool)isBodyHTML {
    env.objc.borrow::<MFMailComposeViewControllerHostObject>(this).body_is_html
}

// MARK: - Attachments (stub — no file I/O in composer)

- (())addAttachmentData:(id)_attachment    // NSData*
              mimeType:(id)mime_type       // NSString*
              fileName:(id)file_name {     // NSString*
    let mime = if mime_type != nil {
        ns_string::to_rust_string(env, mime_type).into_owned()
    } else {
        "(null)".to_string()
    };
    let name = if file_name != nil {
        ns_string::to_rust_string(env, file_name).into_owned()
    } else {
        "(null)".to_string()
    };
    log!(
        "MFMailComposeViewController: addAttachmentData:mimeType:{} fileName:{} — ignored (no mail support)",
        mime, name
    );
}

// MARK: - UIViewController overrides
// When presented we immediately fire the delegate with MFMailComposeResultCancelled
// and dismiss ourselves, since there is no real mail UI in touchHLE.

- (())viewWillAppear:(bool)animated {
    let delegate = env.objc.borrow::<MFMailComposeViewControllerHostObject>(this)
        .mail_compose_delegate;
    if delegate != nil {
        let error: id = nil;
        let _: () = msg![env; delegate mailComposeController:this
                                         didFinishWithResult:MF_MAIL_COMPOSE_RESULT_CANCELLED
                                                       error:error];
    }
}

- (())viewDidAppear:(bool)animated {
    let presenting: id = msg![env; this presentingViewController];
    if presenting != nil {
        let _: () = msg![env; presenting dismissViewControllerAnimated:animated completion:nil];
    }
}

// MARK: - Description

- (id)description {
    let (subject, body_is_html) = {
        let host = env.objc.borrow::<MFMailComposeViewControllerHostObject>(this);
        (host.subject, host.body_is_html)
    }; // immutable borrow ends here

    let subject_str = if subject != nil {
        ns_string::to_rust_string(env, subject).into_owned()
    } else {
        "(null)".to_string()
    };
    let s = format!(
        "<MFMailComposeViewController: subject=\"{}\" bodyIsHTML={}>",
        subject_str, body_is_html
    );
    let cstr = env.mem.alloc_and_write_cstr(s.as_bytes());
    msg_class![env; NSString stringWithUTF8String:cstr]
}

@end

};
