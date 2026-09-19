package com.example;

import javax.inject.Inject;
import javax.inject.Provider;
import javax.inject.Singleton;

@Singleton
class PaymentGateway { void charge() {} }

class AuditLog { void write(String s) {} }

class CheckoutFlow {
    private final PaymentGateway gateway;
    private final Provider<AuditLog> audit;

    @Inject
    CheckoutFlow(PaymentGateway gateway, Provider<AuditLog> audit) {
        this.gateway = gateway; this.audit = audit;
    }
}
