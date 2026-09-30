package shop

import com.launchdarkly.sdk.LDContext
import com.launchdarkly.sdk.server.LDClient

object Checkout {
  def variant(client: LDClient, user: String): String =
    if (client.boolVariation("new-checkout", LDContext.create(user), false)) "new" else "legacy"
}
