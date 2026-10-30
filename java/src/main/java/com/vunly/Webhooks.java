package com.vunly;

import com.vunly.internal.Json;
import java.nio.charset.StandardCharsets;
import java.security.InvalidKeyException;
import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;
import java.time.Duration;
import java.time.Instant;
import java.util.HexFormat;
import java.util.List;
import java.util.Map;
import javax.crypto.Mac;
import javax.crypto.spec.SecretKeySpec;

/** Verifies webhook deliveries. */
public final class Webhooks {
  /** How far a signed timestamp may be from now (either way) before it counts as stale. */
  public static final Duration TOLERANCE = Duration.ofSeconds(300);

  private Webhooks() {}

  /** A verified delivery. {@code data} is the parsed JSON of the event's data (Map, List, String, Long, Double, ...). */
  public record Event(String id, String type, String createdAt, Object data) {}

  /** Thrown by {@link #verify}. Answer a failed verification with a 400 and do not process the payload. */
  public static final class WebhookException extends RuntimeException {
    private static final long serialVersionUID = 1L;
    private final String reason;

    WebhookException(String reason) {
      super("vunly: webhook verification failed (" + reason + ")");
      this.reason = reason;
    }

    /** "missing_header", "malformed_header", "stale_timestamp", "no_matching_signature" or "invalid_payload". */
    public String reason() {
      return reason;
    }
  }

  /** As {@link #verify(byte[], String, List, Instant)} against the current time. */
  public static Event verify(byte[] payload, String header, List<String> secrets) {
    return verify(payload, header, secrets, Instant.now());
  }

  /**
   * Checks a delivery. {@code payload} is the RAW request body (the exact bytes, not re-encoded JSON), {@code header}
   * the {@code Vunly-Signature} header and {@code secrets} the endpoint's signing secrets: pass two while rotating, a
   * match with any one is enough. The signature is HMAC-SHA256 over {@code "<t>." + payload}, compared in constant time.
   */
  public static Event verify(byte[] payload, String header, List<String> secrets, Instant now) {
    // An empty secret signs anything anyone wants: a missing environment variable would make webhooks forgeable. A
    // configuration error, raised before any signature is looked at (a WebhookException would be answered 400 and hide it).
    if (secrets == null
        || secrets.isEmpty()
        || secrets.stream().anyMatch(s -> s == null || s.chars().noneMatch(c -> c >= 0x21 && c <= 0x7e))) {
      throw new IllegalArgumentException(
          "vunly: the webhook secret is empty or missing (is its environment variable set?)");
    }
    if (header == null || header.isBlank()) {
      throw new WebhookException("missing_header");
    }
    String ts = null;
    java.util.ArrayList<String> sigs = new java.util.ArrayList<>();
    for (String part : header.split(",")) {
      int eq = part.indexOf('=');
      if (eq < 0) {
        continue;
      }
      String k = part.substring(0, eq).trim();
      String v = part.substring(eq + 1).trim();
      if (k.equals("t")) {
        ts = v;
      } else if (k.equals("v1") && v.matches("[0-9a-f]{64}")) {
        sigs.add(v); // a v1 that is not 64 hex characters is not a signature: the header is malformed
      }
    }
    long t;
    try {
      t = Long.parseLong(ts == null ? "" : ts);
    } catch (NumberFormatException e) {
      throw new WebhookException("malformed_header");
    }
    if (sigs.isEmpty()) {
      throw new WebhookException("malformed_header");
    }
    if (Duration.between(Instant.ofEpochSecond(t), now).abs().compareTo(TOLERANCE) > 0) {
      throw new WebhookException("stale_timestamp");
    }
    byte[] prefix = (ts + ".").getBytes(StandardCharsets.UTF_8);
    boolean matched = false;
    for (String secret : secrets) {
      byte[] want = hmac(secret, prefix, payload); // fed in two parts: the body is not copied
      for (String s : sigs) {
        byte[] got;
        try {
          got = HexFormat.of().parseHex(s);
        } catch (IllegalArgumentException e) {
          continue;
        }
        if (MessageDigest.isEqual(got, want)) {
          matched = true;
        }
      }
    }
    if (!matched) {
      throw new WebhookException("no_matching_signature");
    }
    try {
      Fields f = Fields.of(Json.parse(payload));
      return new Event(f.str("id"), f.str("type"), f.strOrNull("createdAt"), eventData(Json.parse(payload)));
    } catch (RuntimeException e) {
      throw new WebhookException("invalid_payload");
    }
  }

  private static Object eventData(Object parsed) {
    return parsed instanceof Map<?, ?> m ? m.get("data") : null;
  }

  private static byte[] hmac(String secret, byte[] first, byte[] second) {
    try {
      Mac mac = Mac.getInstance("HmacSHA256");
      mac.init(new SecretKeySpec(secret.getBytes(StandardCharsets.UTF_8), "HmacSHA256"));
      mac.update(first);
      return mac.doFinal(second);
    } catch (NoSuchAlgorithmException | InvalidKeyException e) {
      throw new IllegalStateException("HmacSHA256 is not available", e);
    }
  }
}
