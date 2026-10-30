package com.vunly;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

import com.vunly.internal.Json;
import java.nio.charset.StandardCharsets;
import java.time.Instant;
import java.util.List;
import java.util.Map;
import java.util.stream.Stream;
import org.junit.jupiter.api.DynamicTest;
import org.junit.jupiter.api.TestFactory;

/**
 * The shared webhook vectors (webhook-vectors.json in the test resources, an identical copy in every SDK). The signatures were
 * computed outside any SDK. "expect" is "ok", a WebhookException reason, or "config" (an IllegalArgumentException raised before
 * the signature is looked at).
 */
class WebhookVectorsTest {
  @TestFactory
  @SuppressWarnings("unchecked")
  Stream<DynamicTest> theSharedWebhookVectorsHold() throws Exception {
    Map<String, Object> file;
    try (var in = WebhookVectorsTest.class.getResourceAsStream("/webhook-vectors.json")) {
      file = (Map<String, Object>) Json.parse(in.readAllBytes());
    }
    Instant now = Instant.ofEpochSecond(((Number) file.get("now")).longValue());
    List<Map<String, Object>> vectors = (List<Map<String, Object>>) file.get("vectors");
    assertTrue(vectors.size() >= 20, "only " + vectors.size() + " vectors");
    return vectors.stream()
        .map(
            v ->
                DynamicTest.dynamicTest(
                    v.get("name") + " -> " + v.get("expect"),
                    () -> {
                      String got = "ok";
                      try {
                        Webhooks.verify(
                            ((String) v.get("payload")).getBytes(StandardCharsets.UTF_8),
                            (String) v.get("header"),
                            (List<String>) v.get("secrets"),
                            now);
                      } catch (Webhooks.WebhookException e) {
                        got = e.reason();
                      } catch (IllegalArgumentException e) {
                        got = "config";
                      }
                      assertEquals(v.get("expect"), got);
                    }));
  }
}
