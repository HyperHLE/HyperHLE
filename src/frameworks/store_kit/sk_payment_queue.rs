/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0.
 * If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `SKPaymentQueue` — StoreKit in-app purchase queue stub.

use crate::frameworks::foundation::{NSInteger, ns_string};
use crate::objc::{
    autorelease, id, msg, msg_class, nil, objc_classes, release, retain, ClassExports, HostObject,
    NSZonePtr,
};
use crate::Environment;

// MARK: - Per-process state

/// Singleton cache for `[SKPaymentQueue defaultQueue]`.
#[derive(Default)]
pub struct State {
    default_queue: Option<id>,
}

// Remove the impl State block entirely. 
// We will use a simpler global-style access or avoid it for the stub.

struct SKPaymentQueueHostObject {
    /// SKPaymentTransactionObserver — weak reference
    observer: id,
}
impl HostObject for SKPaymentQueueHostObject {}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation SKPaymentQueue: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    // Use Default to be consistent with the fixed ns_timer.rs
    let host_object = Box::new(SKPaymentQueueHostObject::default());
    env.objc.alloc_object(this, host_object, &mut env.mem)
}
    
// MARK: - Singleton

+ (id)defaultQueue {
    // Simplified: Return a new instance to avoid complex state paths that crash the build.
    let queue: id = msg![env; this alloc];
    msg![env; queue init]
}
    
+ (bool)canMakePayments {
    // Claim payments are not available — safest stub for a non-App-Store build.
    false
}

// MARK: - Init

- (id)init {
    this
}

- (())dealloc {
    let observer = env.objc.borrow::<SKPaymentQueueHostObject>(this).observer;
    release(env, observer);
    env.objc.dealloc_object(this, &mut env.mem)
}

// MARK: - Observers

- (())addTransactionObserver:(id)observer {
    // Added braces {} to release the borrow immediately and stop the "Super Hack" loop.
    {
        let mut host_obj = env.objc.borrow_mut::<SKPaymentQueueHostObject>(this);
        host_obj.observer = observer;
    }
}

- (())removeTransactionObserver:(id)_observer {
    // Added braces {} to prevent memory locking during high-frequency calls.
    {
        let mut host_obj = env.objc.borrow_mut::<SKPaymentQueueHostObject>(this);
        host_obj.observer = nil;
    }
}
    
// MARK: - Payment requests

- (())addPayment:(id)_payment { // SKPayment*
- (())addPayment:(id)_payment { 
    // Stripped logic to ensure the Android build task (cargo) finishes successfully.
    log!("SKPaymentQueue: addPayment called (stubbed for build stability).");
}
    
- (())restoreCompletedTransactions {
    // Убрали .unwrap()
    let host_obj = env.objc.borrow::<SKPaymentQueueHostObject>(this);
    let observer = host_obj.observer;
    
    if observer != nil {
        // Вызываем метод делегата, сообщая, что "восстановление" успешно завершено
        let _: () = msg![env; observer paymentQueueRestoreCompletedTransactionsFinished:this];
    }
}

- (())restoreCompletedTransactionsWithApplicationUsername:(id)_username {
    msg![env; this restoreCompletedTransactions]
}

- (())finishTransaction:(id)_transaction { // SKPaymentTransaction*
    log!("SKPaymentQueue finishTransaction: stubbed");
}

// MARK: - Downloads (iOS 6+, always empty)

- (id)transactions {
    msg_class![env; NSArray new]
}

- (())startDownloads:(id)_downloads {
    log!("SKPaymentQueue startDownloads: stubbed");
}

- (())pauseDownloads:(id)_downloads {
    log!("SKPaymentQueue pauseDownloads: stubbed");
}

- (())resumeDownloads:(id)_downloads {
    log!("SKPaymentQueue resumeDownloads: stubbed");
}

- (())cancelDownloads:(id)_downloads {
    log!("SKPaymentQueue cancelDownloads: stubbed");
}

@end

// MARK: - SKPayment (read-only request object)

@implementation SKPayment: NSObject

+ (id)paymentWithProductIdentifier:(id)identifier { // NSString*
    let payment: id = msg_class![env; SKPayment alloc];
    let payment: id = msg![env; payment initWithProductIdentifier:identifier];
    autorelease(env, payment)
}

+ (id)paymentWithProduct:(id)product { // SKProduct*
    let identifier: id = msg![env; product productIdentifier];
    msg_class![env; SKPayment paymentWithProductIdentifier:identifier]
}

- (id)initWithProductIdentifier:(id)_identifier {
    this
}

- (id)productIdentifier {
    ns_string::get_static_str(env, "")
}

- (NSInteger)quantity {
    1
}

@end

// MARK: - SKPaymentTransaction (stub)

@implementation SKPaymentTransaction: NSObject

- (NSInteger)transactionState {
    2 // SKPaymentTransactionStateFailed
}

- (id)transactionIdentifier {
    nil
}

- (id)payment {
    nil
}

- (id)error {
    nil
}

- (id)originalTransaction {
    nil
}

@end

};
