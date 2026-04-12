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

struct SKPaymentQueueHostObject {
    /// SKPaymentTransactionObserver — weak reference
    observer: id,
}
impl HostObject for SKPaymentQueueHostObject {}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation SKPaymentQueue: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(SKPaymentQueueHostObject {
        observer: nil,
    });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

// MARK: - Singleton

+ (id)defaultQueue {
    // Return a fresh autoreleased stub instance.
    let queue: id = msg![env; this alloc];
    let queue: id = msg![env; queue init];
    autorelease(env, queue)
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
    log!("SKPaymentQueue addTransactionObserver: stubbed (IAP not supported)");

    let old = env.objc.borrow::<SKPaymentQueueHostObject>(this).observer;
    release(env, old);
    retain(env, observer);
    env.objc.borrow_mut::<SKPaymentQueueHostObject>(this).observer = observer;
}

- (())removeTransactionObserver:(id)observer {
    log!("SKPaymentQueue removeTransactionObserver: stubbed");

    let current = env.objc.borrow::<SKPaymentQueueHostObject>(this).observer;
    if current == observer {
        release(env, current);
        env.objc.borrow_mut::<SKPaymentQueueHostObject>(this).observer = nil;
    }
}

// MARK: - Payment requests

- (())addPayment:(id)_payment { // SKPayment*
    log!("SKPaymentQueue addPayment: stubbed — failing transaction immediately");

    // Notify observer that the payment failed so the app can handle it cleanly.
    let observer = env.objc.borrow::<SKPaymentQueueHostObject>(this).observer;

    if observer == nil {
        return;
    }

    // Build a minimal fake SKPaymentTransaction array and call the delegate.
    // Most apps only check the transactionState, which we set to SKPaymentTransactionStateFailed (2).
    let transactions: id = msg_class![env; NSArray new];

    let sel = env.objc.register_host_selector(
        "paymentQueue:updatedTransactions:".to_string(),
        &mut env.mem,
    );

    let responds: bool = msg![env; observer respondsToSelector:sel];
    if responds {
        () = msg![env; observer paymentQueue:this updatedTransactions:transactions];
    }
}

- (())restoreCompletedTransactions {
    log!("SKPaymentQueue restoreCompletedTransactions: stubbed — notifying no transactions");
    let observer = env.objc.borrow::<SKPaymentQueueHostObject>(this).observer;

    if observer == nil {
        return;
    }

    let sel = env.objc.register_host_selector(
        "paymentQueueRestoreCompletedTransactionsFinished:".to_string(),
        &mut env.mem,
    );

    let responds: bool = msg![env; observer respondsToSelector:sel];
    if responds {
        () = msg![env; observer paymentQueueRestoreCompletedTransactionsFinished:this];
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
