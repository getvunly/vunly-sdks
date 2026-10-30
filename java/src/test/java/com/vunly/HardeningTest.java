package com.vunly;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.time.Duration;
import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.atomic.AtomicInteger;
import org.junit.jupiter.api.Test;

/** Regression tests for hardening cases that the shared conformance does not reach. */
class HardeningTest {
  private static final String KEY = TestServer.KEY;

  /** A server error body from a JSON string literal already escaped. */
  private static String error(String code, String message, String requestId, String details) {
    return "{\"error\":{\"code\":\"" + code + "\",\"message\":\"" + message + "\""
        + (requestId == null ? "" : ",\"requestId\":\"" + requestId + "\"")
        + (details == null ? "" : ",\"details\":" + details) + "}}";
  }

  private static boolean unsafe(String s) {
    return s.chars().anyMatch(c -> c < 0x20 || c == 0x7f || (c >= 0x80 && c < 0xa0)
        || (c >= 0x200b && c <= 0x200f) || (c >= 0x202a && c <= 0x202e) || (c >= 0x2066 && c <= 0x2069)
        || c == 0x2028 || c == 0x2029 || c == 0xfeff);
  }

  @Test
  void biDiAndZeroWidthCharactersAreStrippedFromMessageCodeAndRequestId() throws Exception {
    String evil = "\\u202e\\u200b\\u2066\\ufeff\\u2028";
    String body = error("bad" + evil + "_code", "msg" + evil + "end", "req" + evil + "_id12345", null);
    try (TestServer s = new TestServer(ex -> TestServer.json(ex, 400, body))) {
      VunlyException e = assertThrows(VunlyException.class, () -> s.client().build().credits().balance("acme"));
      assertEquals("bad_code", e.code());
      assertEquals("req_id12345", e.requestId());
      assertFalse(unsafe(e.getMessage()), e.getMessage());
      assertTrue(e.getMessage().contains("msgend"), e.getMessage());
    }
  }

  @Test
  void codeAndRequestIdAreBoundedAndMessageToo() throws Exception {
    String body = error("c".repeat(5000), "m".repeat(5000), "r".repeat(5000), null);
    try (TestServer s = new TestServer(ex -> TestServer.json(ex, 400, body))) {
      VunlyException e = assertThrows(VunlyException.class, () -> s.client().build().credits().balance("acme"));
      assertEquals(64, e.code().length());
      assertEquals(128, e.requestId().length());
      assertTrue(e.getMessage().length() < 900, "length " + e.getMessage().length());
    }
  }

  @Test
  void theRequestIdHeaderOfASuccessAndOfAnInvalidResponseIsCleanedAndBounded() throws Exception {
    String header = "req_" + "x".repeat(300);
    try (TestServer s =
        new TestServer(
            ex -> {
              ex.getResponseHeaders().set("Request-Id", header);
              byte[] b = ("{\"event\":" + TestServer.ENTRY + ",\"balance\":97}").getBytes();
              ex.sendResponseHeaders(200, b.length);
              ex.getResponseBody().write(b);
            })) {
      UsageResult ok = s.client().build().usage().record("job-12345678", "acme", "tokens", 3000);
      assertEquals(128, ok.requestId().length());
    }
    try (TestServer s =
        new TestServer(
            ex -> {
              ex.getResponseHeaders().set("Request-Id", header);
              byte[] b = "not json".getBytes();
              ex.sendResponseHeaders(200, b.length);
              ex.getResponseBody().write(b);
            })) {
      UnavailableException e =
          assertThrows(UnavailableException.class, () -> s.client().maxRetries(0).build().credits().balance("acme"));
      assertEquals("invalid_response", e.code());
      assertEquals(128, e.requestId().length());
    }
  }

  @Test
  void stringsInDetailsAreCleanedAndBounded() throws Exception {
    String body =
        error(
            "validation_error",
            "bad",
            null,
            "{\"fields\":{\"id\":\"a\\u202eb\\u200bc\\nd\"},\"list\":[\"x\\u2067y\"],\"long\":\"" + "z".repeat(5000)
                + "\",\"nothing\":null}");
    try (TestServer s = new TestServer(ex -> TestServer.json(ex, 400, body))) {
      VunlyException e = assertThrows(VunlyException.class, () -> s.client().build().credits().balance("acme"));
      String shown = e.details().toString();
      assertFalse(unsafe(shown), shown);
      assertEquals(java.util.Map.of("id", "abcd"), e.details().get("fields"));
      assertEquals(List.of("xy"), e.details().get("list"));
      assertEquals(500, ((String) e.details().get("long")).length());
    }
  }

  /**
   * The server's details are bounded: 50 entries per object or array, and 5 levels (the top object is level 0, so the value under
   * "deep" is level 1 and four nested levels survive; the fifth is cut). Same shape as the Go test.
   */
  @Test
  void detailsAreBoundedInEntriesAndDepth() throws Exception {
    StringBuilder list = new StringBuilder("[");
    StringBuilder many = new StringBuilder("{");
    for (int i = 0; i < 120; i++) {
      list.append(i == 0 ? "" : ",").append(i);
      many.append(i == 0 ? "" : ",").append(String.format("\"k%03d\":%d", i, i));
    }
    list.append("]");
    many.append("}");
    String deepMap = "{\"leaf\":\"x\"}";
    String deepList = "[\"x\"]";
    for (int i = 0; i < 9; i++) {
      deepMap = "{\"n\":" + deepMap + "}";
      deepList = "[" + deepList + "]";
    }
    String details =
        "{\"list\":" + list + ",\"many\":" + many + ",\"deep\":" + deepMap + ",\"deepList\":" + deepList + "}";
    try (TestServer s = new TestServer(ex -> TestServer.json(ex, 400, error("validation_error", "m", null, details)))) {
      VunlyException e = assertThrows(VunlyException.class, () -> s.client().build().credits().balance("acme"));
      List<?> gotList = (List<?>) e.details().get("list");
      assertEquals(50, gotList.size());
      assertEquals(0L, gotList.get(0));
      assertEquals(49L, gotList.get(49));
      java.util.Map<?, ?> gotMany = (java.util.Map<?, ?>) e.details().get("many");
      assertEquals(50, gotMany.size());
      assertTrue(gotMany.containsKey("k000") && gotMany.containsKey("k049"), gotMany.keySet().toString());
      assertFalse(gotMany.containsKey("k050"));
      int levels = 0;
      Object node = e.details().get("deep");
      while (node instanceof java.util.Map<?, ?> m) {
        levels++;
        node = m.get("n");
      }
      assertEquals(4, levels, "object levels that survive");
      assertEquals(null, node);
      int listLevels = 0;
      Object ln = e.details().get("deepList");
      while (ln instanceof List<?> l) {
        listLevels++;
        ln = l.get(0);
      }
      assertEquals(4, listLevels, "array levels that survive");
      assertEquals(null, ln);
    }
  }

  /** A hostile success body (bad timestamp carrying BiDi text, a long run and the key; a body that is not JSON) is never echoed. */
  @Test
  void aHostileSuccessBodyIsNeverEchoedInInvalidResponse() throws Exception {
    String hostile = "\\u202e" + "A".repeat(3000) + KEY;
    String badField = "{\"id\":\"c\",\"externalId\":\"e\",\"createdAt\":\"" + hostile + "\"}";
    String notJson = "{\"id\":\"" + KEY + "‮\u0001";
    for (String body : List.of(badField, notJson)) {
      try (TestServer s = new TestServer(ex -> TestServer.json(ex, 200, body))) {
        VunlyException e = assertThrows(VunlyException.class, () -> s.client().build().customers().upsert("acme"));
        assertEquals("invalid_response", e.code());
        assertFalse(unsafe(e.getMessage()), e.getMessage());
        assertFalse(e.getMessage().contains(KEY), e.getMessage());
        assertFalse(e.getMessage().contains("AAAA"), e.getMessage());
        assertTrue(e.getMessage().length() < 200, "length " + e.getMessage().length());
      }
    }
  }

  @Test
  void anEchoedKeyIsRedactedEverywhereIncludingWhenZeroWidthCharactersSplitIt() throws Exception {
    String split = KEY.substring(0, 10) + "\\u200b" + KEY.substring(10);
    String body =
        error(
            "k_" + KEY.substring(0, 12),
            "your key " + KEY + " and " + split + " is bad",
            "id_" + KEY,
            "{\"echo\":\"" + KEY + "\",\"nested\":[\"" + KEY + "\"]}");
    try (TestServer s =
        new TestServer(
            ex -> {
              ex.getResponseHeaders().set("Request-Id", "hdr_" + KEY);
              TestServer.json(ex, 400, body);
            })) {
      VunlyException e = assertThrows(VunlyException.class, () -> s.client().build().credits().balance("acme"));
      String everything = e.getMessage() + e.code() + e.requestId() + e.details();
      assertFalse(everything.contains(KEY), everything);
      assertTrue(e.getMessage().contains("[redacted]"));
      assertFalse(e.requestId().contains(KEY));
    }
    // the same on the request-id header of a 200 whose body is not JSON
    try (TestServer s =
        new TestServer(
            ex -> {
              ex.getResponseHeaders().set("Request-Id", "hdr_" + KEY);
              byte[] b = "no".getBytes();
              ex.sendResponseHeaders(200, b.length);
              ex.getResponseBody().write(b);
            })) {
      UnavailableException e =
          assertThrows(UnavailableException.class, () -> s.client().maxRetries(0).build().credits().balance("acme"));
      assertFalse(e.requestId().contains(KEY), e.requestId());
    }
  }

  @Test
  void everyRedirectIsUnexpectedRedirectWhateverTheBodySays() throws Exception {
    for (int status : new int[] {301, 302, 303, 307, 308, 304}) {
      try (TestServer s =
          new TestServer(
              ex -> {
                ex.getResponseHeaders().set("Location", "http://127.0.0.1:1/elsewhere");
                TestServer.json(ex, status, error("validation_error", "from the body", null, null));
              })) {
        UnavailableException e =
            assertThrows(UnavailableException.class, () -> s.client().maxRetries(2).build().credits().balance("acme"), "status " + status);
        assertEquals("unexpected_redirect", e.code(), "status " + status);
        assertEquals(status, e.status());
        assertFalse(e.getMessage().contains("from the body"));
        assertEquals(1, s.hits.get(), "not followed, not retried: status " + status);
      }
    }
  }

  @Test
  void theSameIdempotencyKeyGoesOutOnEveryRetryAndRetryAfterIsWaited() throws Exception {
    List<String> keys = new ArrayList<>();
    AtomicInteger n = new AtomicInteger();
    try (TestServer s =
        new TestServer(
            ex -> {
              keys.add(ex.getRequestHeaders().getFirst("Idempotency-Key"));
              if (n.incrementAndGet() < 3) {
                ex.getResponseHeaders().set("Retry-After", "7");
                TestServer.json(ex, 503, error("unavailable", "later", null, null));
              } else {
                TestServer.json(ex, 200, "{\"event\":" + TestServer.ENTRY + ",\"balance\":97}");
              }
            })) {
      List<Duration> waits = new ArrayList<>();
      Vunly v = Vunly.builder().apiKey(KEY).baseUrl(s.url()).maxRetries(3).sleeper(waits::add).build();
      UsageResult r = v.usage().record("job-12345678", "acme", "tokens", 3000);
      assertEquals(97, r.remaining());
      assertEquals(List.of("job-12345678", "job-12345678", "job-12345678"), keys);
      assertEquals(List.of(Duration.ofSeconds(7), Duration.ofSeconds(7)), waits, "Retry-After is waited, not replaced by backoff");
    }
  }

  @Test
  void exactlyThreeEmptyPagesInARowEndTheIteration() throws Exception {
    AtomicInteger n = new AtomicInteger();
    try (TestServer s =
        new TestServer(ex -> TestServer.json(ex, 200, "{\"data\":[],\"nextCursor\":\"c" + n.incrementAndGet() + "\"}"))) {
      Iterable<Customer> all = s.client().build().customers().listAll(10);
      assertThrows(UnavailableException.class, () -> all.iterator().hasNext());
      assertEquals(3, s.hits.get());
    }
    // two empty pages then data is still fine: the count is of pages in a row
    AtomicInteger m = new AtomicInteger();
    try (TestServer s =
        new TestServer(
            ex -> {
              int i = m.incrementAndGet();
              if (i <= 2) {
                TestServer.json(ex, 200, "{\"data\":[],\"nextCursor\":\"c" + i + "\"}");
              } else {
                TestServer.json(ex, 200, "{\"data\":[{\"id\":\"cus_1\",\"externalId\":\"acme\",\"createdAt\":\"2026-01-01T00:00:00Z\"}]}");
              }
            })) {
      assertTrue(s.client().build().customers().listAll(10).iterator().hasNext());
      assertEquals(3, s.hits.get());
    }
  }
}
