package com.example;

import com.launchdarkly.sdk.LDContext;
import com.launchdarkly.sdk.server.LDClient;

public class Checkout {
    public String variant(LDClient client, String userKey) {
        boolean on = client.boolVariation("new-checkout", LDContext.create(userKey), false);
        return on ? "new" : "legacy";
    }
}
