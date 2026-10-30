package com.vunly;

import java.nio.charset.StandardCharsets;
import java.util.HexFormat;
import javax.crypto.Mac;
import javax.crypto.spec.SecretKeySpec;

/** Builds a Vunly-Signature header for tests. */
final class WebhookSigning {
  private WebhookSigning() {}

  static String header(String secret, long t, String payload) {
    try {
      Mac mac = Mac.getInstance("HmacSHA256");
      mac.init(new SecretKeySpec(secret.getBytes(StandardCharsets.UTF_8), "HmacSHA256"));
      byte[] sig = mac.doFinal((t + "." + payload).getBytes(StandardCharsets.UTF_8));
      return "t=" + t + ",v1=" + HexFormat.of().formatHex(sig);
    } catch (java.security.GeneralSecurityException e) {
      throw new IllegalStateException(e);
    }
  }
}
