package com.vunly;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertInstanceOf;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.io.BufferedReader;
import java.io.IOException;
import java.io.InputStreamReader;
import java.io.OutputStream;
import java.net.InetAddress;
import java.net.ServerSocket;
import java.net.Socket;
import java.nio.charset.StandardCharsets;
import java.time.Duration;
import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.atomic.AtomicInteger;
import org.junit.jupiter.api.Test;

class VunlyTest {
  private static UsageResult record(Vunly v) {
    return v.usage().record("job-12345678", "acme", "tokens", 3000);
  }

  @Test
  void builderRequiresKeyAndAddressAndNeverPrintsTheKey() {
    assertThrows(IllegalArgumentException.class, () -> Vunly.builder().baseUrl("http://127.0.0.1:1").build());
    assertThrows(IllegalArgumentException.class, () -> Vunly.builder().apiKey("k").build());
    assertThrows(IllegalArgumentException.class, () -> Vunly.builder().apiKey("k").baseUrl("not a url").build());
    assertThrows(IllegalArgumentException.class, () -> Vunly.builder().apiKey("k").baseUrl("ftp://host").build());
    Vunly v = Vunly.builder().apiKey(TestServer.KEY).baseUrl("http://127.0.0.1:1").build();
    assertFalse(v.toString().contains(TestServer.KEY));
    assertFalse(String.valueOf(v).contains(TestServer.KEY));
  }

  @Test
  void baseUrlAcceptsTrailingSlashAndV1() {
    assertEquals("https://api.example.test", Vunly.normalizeBaseUrl("https://api.example.test/v1/"));
    assertEquals("https://api.example.test", Vunly.normalizeBaseUrl("https://api.example.test/"));
    assertEquals("https://api.example.test", Vunly.normalizeBaseUrl("  https://api.example.test/V1 "));
  }

  @Test
  void recordSendsKeyAndIdempotencyAndReportsOverage() throws Exception {
    try (TestServer s =
        new TestServer(
            ex -> {
              assertEquals("Bearer " + TestServer.KEY, ex.getRequestHeaders().getFirst("Authorization"));
              assertEquals("job-12345678", ex.getRequestHeaders().getFirst("Idempotency-Key"));
              assertTrue(ex.getRequestHeaders().getFirst("User-Agent").startsWith("vunly-java/"));
              assertTrue(TestServer.bodyOf(ex).contains("\"quantity\":3000"));
              ex.getResponseHeaders().set("Vunly-Plan-Overage", "true");
              TestServer.json(ex, 200, "{\"event\":" + TestServer.ENTRY + ",\"balance\":97}");
            })) {
      UsageResult r = record(s.client().build());
      assertEquals(97, r.remaining());
      assertTrue(r.planOverage());
      assertFalse(r.replayed());
      assertEquals("req_test", r.requestId());
      assertEquals(-3, r.event().credits());
    }
  }

  @Test
  void replayIsReportedAndNeverFlagsOverage() throws Exception {
    try (TestServer s =
        new TestServer(
            ex -> {
              ex.getResponseHeaders().set("Idempotent-Replayed", "true");
              ex.getResponseHeaders().set("Vunly-Plan-Overage", "true");
              TestServer.json(ex, 200, "{\"event\":" + TestServer.ENTRY + ",\"balance\":97}");
            })) {
      UsageResult r = record(s.client().build());
      assertTrue(r.replayed());
      assertFalse(r.planOverage());
    }
  }

  @Test
  void badIdIsRefusedBeforeAnyRequest() throws Exception {
    try (TestServer s = new TestServer(ex -> TestServer.json(ex, 200, "{}"))) {
      Vunly v = s.client().build();
      InvalidRequestException e =
          assertThrows(InvalidRequestException.class, () -> v.usage().record("short", "a", "m", 1));
      assertEquals(400, e.status());
      assertEquals("validation_error", e.code());
      assertEquals(0, s.hits.get());
    }
  }

  @Test
  void limitReachedCarriesBalanceAndIsNotRetried() throws Exception {
    try (TestServer s =
        new TestServer(
            ex ->
                TestServer.json(
                    ex,
                    402,
                    "{\"error\":{\"code\":\"insufficient_credits\",\"message\":\"not enough\",\"details\":{\"balance\":2,\"required\":5}}}"))) {
      LimitReachedException e = assertThrows(LimitReachedException.class, () -> record(s.client().build()));
      assertEquals(2, e.balance().getAsLong());
      assertEquals(5, e.required().getAsLong());
      assertEquals("req_test", e.requestId());
      assertEquals("insufficient_credits", e.code());
      assertEquals(1, s.hits.get());
      assertFalse(e.getMessage().contains(TestServer.KEY));
    }
  }

  @Test
  void authAndInvalidRequestClasses() throws Exception {
    int[] statuses = {401, 403, 400, 404, 409};
    for (int status : statuses) {
      try (TestServer s =
          new TestServer(ex -> TestServer.json(ex, status, "{\"error\":{\"code\":\"x\",\"message\":\"m\"}}"))) {
        Vunly v = s.client().build();
        VunlyException e = assertThrows(VunlyException.class, () -> v.customers().get("acme"));
        if (status == 401 || status == 403) {
          assertInstanceOf(AuthException.class, e, "status " + status);
        } else {
          assertInstanceOf(InvalidRequestException.class, e, "status " + status);
        }
        assertEquals(1, s.hits.get(), "no retry for " + status);
      }
    }
  }

  @Test
  void readsRetryServerErrorsThenSucceed() throws Exception {
    AtomicInteger n = new AtomicInteger();
    try (TestServer s =
        new TestServer(
            ex -> {
              if (n.incrementAndGet() < 3) {
                TestServer.json(ex, 503, "{\"error\":{\"code\":\"unavailable\",\"message\":\"later\"}}");
              } else {
                TestServer.json(ex, 200, "{\"credits\":5,\"consumed\":1,\"granted\":6}");
              }
            })) {
      Balance b = s.client().build().credits().balance("acme");
      assertEquals(5, b.remaining());
      assertEquals(3, s.hits.get());
    }
  }

  @Test
  void retriesAreBounded() throws Exception {
    try (TestServer s =
        new TestServer(ex -> TestServer.json(ex, 500, "{\"error\":{\"code\":\"internal_error\",\"message\":\"boom\"}}"))) {
      Vunly v = s.client().maxRetries(2).build();
      assertThrows(UnavailableException.class, () -> v.credits().balance("acme"));
      assertEquals(3, s.hits.get());
    }
  }

  @Test
  void maxRetriesZeroMeansOneAttempt() throws Exception {
    try (TestServer s =
        new TestServer(ex -> TestServer.json(ex, 500, "{\"error\":{\"code\":\"internal_error\",\"message\":\"boom\"}}"))) {
      Vunly v = s.client().maxRetries(0).build();
      assertThrows(UnavailableException.class, () -> v.credits().balance("acme"));
      assertEquals(1, s.hits.get());
    }
  }

  @Test
  void classXIsRetriedOnlyAfter429() throws Exception {
    try (TestServer s =
        new TestServer(ex -> TestServer.json(ex, 500, "{\"error\":{\"code\":\"internal_error\",\"message\":\"boom\"}}"))) {
      assertThrows(UnavailableException.class, () -> s.client().build().customerTokens().create("acme"));
      assertEquals(1, s.hits.get());
    }
    AtomicInteger n = new AtomicInteger();
    try (TestServer s =
        new TestServer(
            ex -> {
              if (n.incrementAndGet() == 1) {
                ex.getResponseHeaders().set("Retry-After", "1");
                TestServer.json(ex, 429, "{\"error\":{\"code\":\"rate_limited\",\"message\":\"slow\"}}");
              } else {
                TestServer.json(
                    ex,
                    200,
                    "{\"id\":\"ct_1\",\"token\":\"vct_secret_value\",\"customerId\":\"cus_1\",\"expiresAt\":\"2026-01-01T00:15:00Z\"}");
              }
            })) {
      CustomerToken t = s.client().build().customerTokens().create("acme");
      assertEquals(2, s.hits.get());
      assertFalse(t.toString().contains("vct_secret_value"));
      assertEquals("vct_secret_value", t.token());
    }
  }

  @Test
  void planVolumeStopIsNeverRetried() throws Exception {
    try (TestServer s =
        new TestServer(
            ex -> {
              ex.getResponseHeaders().set("Retry-After", "5");
              TestServer.json(
                  ex,
                  429,
                  "{\"error\":{\"code\":\"rate_limited\",\"message\":\"plan\",\"details\":{\"reason\":\"plan_included_volume\"}}}");
            })) {
      UnavailableException e = assertThrows(UnavailableException.class, () -> record(s.client().build()));
      assertTrue(e.isPlanStop());
      assertEquals(1, s.hits.get());
    }
  }

  @Test
  void retryAfterAboveTheCapIsSurfaced() throws Exception {
    try (TestServer s =
        new TestServer(
            ex -> {
              ex.getResponseHeaders().set("Retry-After", "120");
              TestServer.json(ex, 429, "{\"error\":{\"code\":\"rate_limited\",\"message\":\"slow\"}}");
            })) {
      UnavailableException e =
          assertThrows(UnavailableException.class, () -> s.client().build().credits().balance("acme"));
      assertEquals(Duration.ofSeconds(120), e.retryAfter());
      assertEquals(1, s.hits.get());
    }
  }

  @Test
  void backoffStaysInItsBounds() {
    for (int attempt = 1; attempt <= 10; attempt++) {
      long cap = Math.min(8000L, 250L << (attempt - 1));
      for (int i = 0; i < 200; i++) {
        long ms = Transport.retryDelay(attempt, null).toMillis();
        assertTrue(ms >= cap / 2 && ms <= cap, "attempt " + attempt + " waited " + ms);
      }
    }
  }

  @Test
  void malformedSuccessIsInvalidResponse() throws Exception {
    try (TestServer s = new TestServer(ex -> TestServer.json(ex, 200, "{\"nope\":1}"))) {
      UnavailableException e =
          assertThrows(UnavailableException.class, () -> s.client().maxRetries(0).build().credits().balance("acme"));
      assertEquals("invalid_response", e.code());
    }
    try (TestServer s = new TestServer(ex -> TestServer.json(ex, 200, "not json"))) {
      UnavailableException e =
          assertThrows(UnavailableException.class, () -> s.client().build().credits().balance("acme"));
      assertEquals("invalid_response", e.code());
      assertEquals(1, s.hits.get(), "an invalid response is not retried");
    }
  }

  @Test
  void aMissingRequiredFieldNamesItInTheMessage() throws Exception {
    try (TestServer s =
        new TestServer(ex -> TestServer.json(ex, 200, "{\"event\":{\"id\":\"led_1\"},\"balance\":1}"))) {
      UnavailableException e = assertThrows(UnavailableException.class, () -> record(s.client().build()));
      assertEquals("invalid_response", e.code());
      assertTrue(e.getMessage().contains("event.customerId"), e.getMessage());
    }
  }

  @Test
  void unknownFieldsAreIgnored() throws Exception {
    try (TestServer s =
        new TestServer(
            ex -> TestServer.json(ex, 200, "{\"credits\":5,\"consumed\":1,\"granted\":6,\"newField\":{\"a\":1}}"))) {
      assertEquals(5, s.client().build().credits().balance("acme").remaining());
    }
  }

  @Test
  void timeoutIsUnavailable() throws Exception {
    try (TestServer s =
        new TestServer(
            ex -> {
              try {
                Thread.sleep(2000);
              } catch (InterruptedException ignored) {
                Thread.currentThread().interrupt();
              }
            })) {
      Vunly v = s.client().timeout(Duration.ofMillis(100)).maxRetries(0).build();
      long start = System.nanoTime();
      UnavailableException e = assertThrows(UnavailableException.class, () -> v.credits().balance("acme"));
      assertTrue(Duration.ofNanos(System.nanoTime() - start).toMillis() < 1500);
      assertEquals("unavailable", e.code());
    }
  }

  @Test
  void interruptionStopsTheCallAndRestoresTheFlag() throws Exception {
    try (TestServer s =
        new TestServer(
            ex -> {
              try {
                Thread.sleep(2000);
              } catch (InterruptedException ignored) {
                Thread.currentThread().interrupt();
              }
            })) {
      Vunly v = s.client().build();
      Thread.currentThread().interrupt();
      try {
        UnavailableException e = assertThrows(UnavailableException.class, () -> v.credits().balance("acme"));
        assertEquals("cancelled", e.code());
        assertTrue(Thread.currentThread().isInterrupted());
      } finally {
        Thread.interrupted(); // clear the flag for the next test
      }
    }
  }

  @Test
  void redirectsAreNotFollowed() throws Exception {
    AtomicInteger followed = new AtomicInteger();
    try (TestServer target = new TestServer(ex -> followed.incrementAndGet())) {
      try (TestServer s =
          new TestServer(
              ex -> {
                ex.getResponseHeaders().set("Location", target.url() + "/x");
                ex.sendResponseHeaders(302, -1);
              })) {
        assertThrows(VunlyException.class, () -> s.client().build().credits().balance("acme"));
        assertEquals(0, followed.get());
      }
    }
  }

  @Test
  void pagingIsLazy() throws Exception {
    AtomicInteger pages = new AtomicInteger();
    try (TestServer s =
        new TestServer(
            ex -> {
              int n = pages.incrementAndGet();
              String next = n < 3 ? "\"c" + n + "\"" : "null";
              TestServer.json(
                  ex, 200, "{\"data\":[" + TestServer.ENTRY + "," + TestServer.ENTRY + "],\"nextCursor\":" + next + "}");
            })) {
      Vunly v = s.client().build();
      int count = 0;
      for (LedgerEntry e : v.credits().ledgerAll("acme", 2)) {
        assertNotNull(e.id());
        if (++count == 3) {
          break;
        }
      }
      assertEquals(3, count);
      assertEquals(2, pages.get(), "stopping early stops fetching");
      pages.set(0);
      int all = 0;
      for (LedgerEntry ignored : v.credits().ledgerAll("acme", 2)) {
        all++;
      }
      assertEquals(6, all);
      assertEquals(3, pages.get());
    }
  }

  @Test
  void pathSegmentsAreEscaped() throws Exception {
    try (TestServer s =
        new TestServer(
            ex -> {
              assertEquals("/v1/customers/a%2Fb%3Fc/balance", ex.getRequestURI().getRawPath());
              TestServer.json(ex, 200, "{\"credits\":0,\"consumed\":0,\"granted\":0}");
            })) {
      s.client().build().credits().balance("a/b?c");
    }
  }

  /**
   * The JDK client must not send a keyed POST twice behind the SDK's back after a dropped connection. A raw socket
   * server answers the first GET with keep-alive and then drops every POST after reading it.
   */
  @Test
  void aDroppedKeyedRequestIsNotReplayedByTheTransport() throws Exception {
    AtomicInteger posts = new AtomicInteger();
    try (ServerSocket server = new ServerSocket(0, 10, InetAddress.getLoopbackAddress())) {
      Thread t =
          new Thread(
              () -> {
                try {
                  while (true) {
                    Socket c = server.accept();
                    serve(c, posts);
                  }
                } catch (IOException ignored) {
                  // closed by the test
                }
              });
      t.setDaemon(true);
      t.start();
      Vunly v =
          Vunly.builder()
              .apiKey(TestServer.KEY)
              .baseUrl("http://127.0.0.1:" + server.getLocalPort())
              .maxRetries(0)
              .build();
      assertEquals(1, v.credits().balance("acme").remaining()); // warms a keep-alive connection
      assertThrows(UnavailableException.class, () -> record(v));
      assertEquals(1, posts.get(), "exactly one POST reached the server");
    }
  }

  /** Many servers drop a connection that asks to upgrade to h2c; the JDK client asks by default on cleartext HTTP. */
  @Test
  void cleartextRequestsDoNotAskForAnH2cUpgrade() throws Exception {
    List<String> seen = new java.util.concurrent.CopyOnWriteArrayList<>();
    try (ServerSocket server = new ServerSocket(0, 10, InetAddress.getLoopbackAddress())) {
      Thread t =
          new Thread(
              () -> {
                try (Socket c = server.accept()) {
                  BufferedReader in = new BufferedReader(new InputStreamReader(c.getInputStream(), StandardCharsets.UTF_8));
                  int length = 0;
                  for (String h = in.readLine(); h != null && !h.isEmpty(); h = in.readLine()) {
                    seen.add(h.toLowerCase(java.util.Locale.ROOT));
                    if (h.toLowerCase(java.util.Locale.ROOT).startsWith("content-length:")) {
                      length = Integer.parseInt(h.substring(15).trim());
                    }
                  }
                  for (int i = 0; i < length; i++) {
                    in.read();
                  }
                  byte[] body = "{\"credits\":1,\"consumed\":0,\"granted\":1}".getBytes(StandardCharsets.UTF_8);
                  OutputStream out = c.getOutputStream();
                  out.write(
                      ("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: " + body.length + "\r\n\r\n")
                          .getBytes(StandardCharsets.UTF_8));
                  out.write(body);
                  out.flush();
                } catch (IOException ignored) {
                  // closed by the test
                }
              });
      t.setDaemon(true);
      t.start();
      Vunly.builder()
          .apiKey(TestServer.KEY)
          .baseUrl("http://127.0.0.1:" + server.getLocalPort())
          .build()
          .credits()
          .balance("acme");
      assertFalse(seen.stream().anyMatch(h -> h.startsWith("upgrade:") || h.startsWith("http2-settings:")), seen.toString());
    }
  }

  private static void serve(Socket c, AtomicInteger posts) {
    Thread th =
        new Thread(
            () -> {
              try (c) {
                BufferedReader in = new BufferedReader(new InputStreamReader(c.getInputStream(), StandardCharsets.UTF_8));
                OutputStream out = c.getOutputStream();
                while (true) {
                  String line = in.readLine();
                  if (line == null) {
                    return;
                  }
                  boolean post = line.startsWith("POST");
                  int length = 0;
                  List<String> headers = new ArrayList<>();
                  for (String h = in.readLine(); h != null && !h.isEmpty(); h = in.readLine()) {
                    headers.add(h);
                    if (h.toLowerCase(java.util.Locale.ROOT).startsWith("content-length:")) {
                      length = Integer.parseInt(h.substring(15).trim());
                    }
                  }
                  for (int i = 0; i < length; i++) {
                    in.read();
                  }
                  if (post) {
                    posts.incrementAndGet();
                    return; // the request arrived, the answer never will
                  }
                  byte[] body = "{\"credits\":1,\"consumed\":0,\"granted\":1}".getBytes(StandardCharsets.UTF_8);
                  out.write(
                      ("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: "
                              + body.length
                              + "\r\nConnection: keep-alive\r\n\r\n")
                          .getBytes(StandardCharsets.UTF_8));
                  out.write(body);
                  out.flush();
                }
              } catch (IOException ignored) {
                // the client went away
              }
            });
    th.setDaemon(true);
    th.start();
  }

  @Test
  void webhooksVerifyAndRejectLikeTheOtherSdks() {
    long now = 1_800_000_000L;
    java.time.Instant at = java.time.Instant.ofEpochSecond(now);
    String payload = "{\"id\":\"evt_1\",\"type\":\"credits.low\",\"createdAt\":\"2026-01-01T00:00:00Z\",\"data\":{\"a\":1}}";
    byte[] body = payload.getBytes(StandardCharsets.UTF_8);

    Webhooks.Event ev = Webhooks.verify(body, WebhookSigning.header("old", now, payload), List.of("new", "old"), at);
    assertEquals("evt_1", ev.id());
    assertEquals("credits.low", ev.type());

    assertEquals("missing_header", reason(body, "", List.of("s"), at));
    assertEquals("malformed_header", reason(body, "garbage", List.of("s"), at));
    assertEquals("stale_timestamp", reason(body, WebhookSigning.header("s", now - 301, payload), List.of("s"), at));
    assertEquals("stale_timestamp", reason(body, WebhookSigning.header("s", now + 301, payload), List.of("s"), at));
    assertEquals("no_matching_signature", reason(body, WebhookSigning.header("other", now, payload), List.of("s"), at));
    assertEquals(
        "no_matching_signature",
        reason((payload + " ").getBytes(StandardCharsets.UTF_8), WebhookSigning.header("s", now, payload), List.of("s"), at));
    String bad = "{\"nope\":true}";
    assertEquals(
        "invalid_payload",
        reason(bad.getBytes(StandardCharsets.UTF_8), WebhookSigning.header("s", now, bad), List.of("s"), at));
  }

  /** R-FM-2: a server that never stops sending is cut at 16 MB; the SDK's own error, not retried, the rest never read. */
  @Test
  void aResponseOverTheCapIsInvalidResponseNotRetriedAndNotReadToTheEnd() throws Exception {
    AtomicInteger completed = new AtomicInteger();
    byte[] chunk = new byte[1 << 20];
    java.util.Arrays.fill(chunk, (byte) ' ');
    try (TestServer s =
        new TestServer(
            ex -> {
              ex.getResponseHeaders().set("Content-Type", "application/json");
              ex.sendResponseHeaders(200, 64L << 20);
              try (OutputStream os = ex.getResponseBody()) {
                for (int i = 0; i < 64; i++) {
                  os.write(chunk);
                  os.flush();
                }
                completed.incrementAndGet();
              } catch (IOException clientStoppedReading) {
                // that is the point
              }
            })) {
      Vunly v = Vunly.builder().apiKey(TestServer.KEY).baseUrl(s.url()).maxRetries(2).sleeper(d -> {}).build();
      UnavailableException e = assertThrows(UnavailableException.class, () -> v.credits().balance("acme"));
      assertEquals("invalid_response", e.code());
      assertEquals(200, e.status());
      assertTrue(e.getMessage().contains("16 MB"));
      assertFalse(e.getMessage().contains(TestServer.KEY));
      assertEquals(1, s.hits.get(), "an oversized success is not retried");
      Thread.sleep(300);
      assertEquals(0, completed.get(), "the client kept reading to the end of the body");
      assertEquals(16 * 1024 * 1024, Transport.MAX_RESPONSE_BYTES);
    }
  }

  // ---- regression tests for edge cases -------------------------------------------------------------------------------------------

  @Test
  void aNullIdIsRefusedBeforeAnyRequestForKeyedMutations() throws Exception {
    try (TestServer s = new TestServer(ex -> TestServer.json(ex, 200, "{}"))) {
      Vunly v = s.client().maxRetries(3).build();
      InvalidRequestException a = assertThrows(InvalidRequestException.class, () -> v.usage().record(null, "acme", "tokens", 1));
      assertEquals("validation_error", a.code());
      assertTrue(a.getMessage().contains("id is required"));
      assertThrows(InvalidRequestException.class, () -> v.credits().grant(null, "acme", 5));
      assertEquals(0, s.hits.get(), "no request left the process, so nothing was retried either");
    }
  }

  @Test
  void aKeyWithATrailingNewlineIsTrimmedAndAKeyThatCannotBeSentNeverLeaks() throws Exception {
    try (TestServer s =
        new TestServer(
            ex -> {
              assertEquals("Bearer " + TestServer.KEY, ex.getRequestHeaders().getFirst("Authorization"));
              TestServer.json(ex, 200, "{\"credits\":1,\"consumed\":0,\"granted\":1}");
            })) {
      Vunly v = Vunly.builder().apiKey(TestServer.KEY + "\n").baseUrl(s.url()).build();
      assertEquals(1, v.credits().balance("acme").remaining());
      assertFalse(v.toString().contains(TestServer.KEY));
    }
    for (String bad : new String[] {"vk_test_synthetic_inner space_0000000001", "vk_test_synthetic_in\nner_0000000001", "vk_test_synthetic_é_0000000001"}) {
      IllegalArgumentException e = assertThrows(IllegalArgumentException.class, () -> Vunly.builder().apiKey(bad).baseUrl("http://127.0.0.1:1").build());
      assertFalse(e.getMessage().contains("synthetic"), "the message quotes no part of the key");
      assertFalse(e.getMessage().contains("inner"));
    }
  }

  @Test
  void anHttpClientThatFollowsRedirectsIsRefusedAndOneThatDoesNotIsAccepted() {
    java.net.http.HttpClient follows = java.net.http.HttpClient.newBuilder().followRedirects(java.net.http.HttpClient.Redirect.ALWAYS).build();
    assertThrows(IllegalArgumentException.class, () -> Vunly.builder().apiKey(TestServer.KEY).baseUrl("http://127.0.0.1:1").httpClient(follows).build());
    java.net.http.HttpClient never = java.net.http.HttpClient.newBuilder().followRedirects(java.net.http.HttpClient.Redirect.NEVER).build();
    assertNotNull(Vunly.builder().apiKey(TestServer.KEY).baseUrl("http://127.0.0.1:1").httpClient(never).build());
  }

  @Test
  void aRedirectIsNotFollowedNotRetriedAndNotCalledValidationError() throws Exception {
    try (TestServer s =
        new TestServer(
            ex -> {
              ex.getResponseHeaders().set("Location", "http://127.0.0.1:1/elsewhere");
              ex.sendResponseHeaders(302, -1);
            })) {
      UnavailableException e = assertThrows(UnavailableException.class, () -> s.client().maxRetries(2).build().credits().balance("acme"));
      assertEquals("unexpected_redirect", e.code());
      assertEquals(302, e.status());
      assertEquals(1, s.hits.get());
    }
  }

  @Test
  void textTheServerSendsIsCleanedOfControlCharactersAndBounded() throws Exception {
    String evil = "{\"error\":{\"code\":\"validation_error\",\"message\":\"bad\\r\\n2026-10-28 INFO forged line\\u001b[31m\",\"requestId\":\"req\\nforged\"}}";
    try (TestServer s = new TestServer(ex -> TestServer.json(ex, 400, evil))) {
      InvalidRequestException e = assertThrows(InvalidRequestException.class, () -> s.client().build().credits().balance("acme"));
      for (char c : new char[] {'\r', '\n', '\u001b'}) {
        assertFalse(e.getMessage().indexOf(c) >= 0, "control character " + (int) c + " in " + e.getMessage());
      }
      assertFalse(e.requestId().contains("\n"));
    }
    String huge = "{\"error\":{\"code\":\"validation_error\",\"message\":\"" + "x".repeat(100_000) + "\"}}";
    try (TestServer s = new TestServer(ex -> TestServer.json(ex, 400, huge))) {
      InvalidRequestException e = assertThrows(InvalidRequestException.class, () -> s.client().build().credits().balance("acme"));
      assertTrue(e.getMessage().length() < 700, "length " + e.getMessage().length());
    }
  }

  @Test
  void retryAfterIsPlainSecondsAndTheReportedValueIsBounded() throws Exception {
    java.util.Map<String, Duration> cases = new java.util.LinkedHashMap<>();
    cases.put("5", Duration.ofSeconds(5));
    cases.put("1.5", Duration.ofMillis(1500));
    cases.put("999999999", Duration.ofDays(1));
    cases.put("0x1p3", null);
    cases.put("1d", null);
    cases.put("99999999999999999999", Duration.ofDays(1));
    cases.put("NaN", null);
    cases.put("-3", null);
    for (var c : cases.entrySet()) {
      try (TestServer s =
          new TestServer(
              ex -> {
                ex.getResponseHeaders().set("Retry-After", c.getKey());
                TestServer.json(ex, 429, "{\"error\":{\"code\":\"rate_limited\",\"message\":\"slow\"}}");
              })) {
        UnavailableException e = assertThrows(UnavailableException.class, () -> s.client().maxRetries(0).build().credits().balance("acme"));
        assertEquals(c.getValue(), e.retryAfter(), "Retry-After: " + c.getKey());
      }
    }
  }

  /**
   * The shared vectors (conformance/retry-after-vectors.json, copied to test resources because the module is built alone in its
   * container). A number above the 30 s cap, or too large for a double ("1e999", hundreds of digits), is a very long wait and is
   * surfaced at once, never retried early; a number up to 30 is the wait; anything else is no header.
   */
  @Test
  @SuppressWarnings("unchecked")
  void theSharedRetryAfterVectorsHold() throws Exception {
    java.util.Map<String, Object> file;
    try (var in = VunlyTest.class.getResourceAsStream("/retry-after-vectors.json")) {
      file = (java.util.Map<String, Object>) com.vunly.internal.Json.parse(in.readAllBytes());
    }
    java.util.List<java.util.Map<String, Object>> vectors = (java.util.List<java.util.Map<String, Object>>) file.get("vectors");
    assertTrue(vectors.size() >= 40, "only " + vectors.size() + " vectors");
    for (var v : vectors) {
      String header = (String) v.get("header");
      String kind = (String) v.get("kind");
      Duration parsed = Transport.parseRetryAfter(header);
      String shown = header.length() > 30 ? header.substring(0, 30) + "..." : header;
      switch (kind) {
        case "ignored" -> assertEquals(null, parsed, "Retry-After '" + shown + "' should mean none");
        default -> {
          double secs = ((Number) v.get("seconds")).doubleValue();
          assertEquals(Duration.ofMillis((long) (secs * 1000)), parsed, "Retry-After '" + shown + "'");
        }
      }
      if ("surface".equals(kind) && header.chars().allMatch(c -> c < 0x80)) {
        // end to end: one request, the 429 comes back at once with the capped value, no early retry
        try (TestServer s =
            new TestServer(
                ex -> {
                  ex.getResponseHeaders().set("Retry-After", header);
                  TestServer.json(ex, 429, "{\"error\":{\"code\":\"rate_limited\",\"message\":\"slow\"}}");
                })) {
          UnavailableException e = assertThrows(UnavailableException.class, () -> s.client().maxRetries(2).build().credits().balance("acme"));
          assertEquals(1, s.hits.get(), "Retry-After '" + shown + "' was retried early");
          assertEquals(parsed, e.retryAfter());
        }
      }
    }
  }

  @Test
  void aBaseUrlWithCredentialsIsRefused() {
    IllegalArgumentException e = assertThrows(IllegalArgumentException.class, () -> Vunly.builder().apiKey(TestServer.KEY).baseUrl("https://user:hunter2@api.example.test").build());
    assertFalse(e.getMessage().contains("hunter2"));
    assertThrows(IllegalArgumentException.class, () -> Vunly.builder().apiKey(TestServer.KEY).baseUrl("https://api.example.test/?token=x").build());
  }

  @Test
  void aCursorThatDoesNotAdvanceEndsTheIterationInsteadOfLoopingForever() throws Exception {
    try (TestServer s = new TestServer(ex -> TestServer.json(ex, 200, "{\"data\":[],\"nextCursor\":\"same\"}"))) {
      Iterable<Customer> all = s.client().build().customers().listAll(10);
      UnavailableException e = assertThrows(UnavailableException.class, () -> all.iterator().hasNext());
      assertEquals("invalid_response", e.code());
      assertTrue(s.hits.get() <= 3, "stopped after " + s.hits.get() + " requests");
    }
  }

  @Test
  void anIdThatWouldChangeTheRouteIsRefusedBeforeAnyRequest() throws Exception {
    try (TestServer s = new TestServer(ex -> TestServer.json(ex, 200, "{}"))) {
      Vunly v = s.client().build();
      for (String bad : new String[] {"", ".", ".."}) {
        assertThrows(InvalidRequestException.class, () -> v.customers().get(bad), "customers().get(\"" + bad + "\")");
        assertThrows(InvalidRequestException.class, () -> v.credits().balance(bad), "credits().balance(\"" + bad + "\")");
        assertThrows(InvalidRequestException.class, () -> v.customerTokens().revoke(bad), "revoke(\"" + bad + "\")");
      }
      assertEquals(0, s.hits.get(), "nothing was sent");
    }
  }

  @Test
  void emptyPagesWithAFreshCursorEachTimeEndTheIterationToo() throws Exception {
    java.util.concurrent.atomic.AtomicInteger n = new java.util.concurrent.atomic.AtomicInteger();
    try (TestServer s = new TestServer(ex -> TestServer.json(ex, 200, "{\"data\":[],\"nextCursor\":\"c" + n.incrementAndGet() + "\"}"))) {
      Iterable<Customer> all = s.client().build().customers().listAll(10);
      UnavailableException e = assertThrows(UnavailableException.class, () -> all.iterator().hasNext());
      assertEquals("invalid_response", e.code());
      assertEquals(3, s.hits.get(), "stops at exactly the third empty page in a row");
    }
  }

  @Test
  void webhookVerificationRefusesAnEmptySecretInsteadOfAcceptingAForgery() {
    String payload = "{\"id\":\"evt_1\",\"type\":\"webhook.test\",\"createdAt\":\"2026-10-28T00:00:00Z\",\"data\":{}}";
    long t = 1_800_000_000L;
    String header = WebhookSigning.header("whsec_synthetic_real", t, payload);
    java.time.Instant at = java.time.Instant.ofEpochSecond(t);
    byte[] body = payload.getBytes(StandardCharsets.UTF_8);
    List<List<String>> bad = new ArrayList<>();
    bad.add(null);
    bad.add(List.of());
    bad.add(List.of(""));
    bad.add(List.of("   "));
    bad.add(List.of("whsec_synthetic_real", ""));
    bad.add(java.util.Arrays.asList("whsec_synthetic_real", null));
    for (List<String> secrets : bad) {
      IllegalArgumentException e = assertThrows(IllegalArgumentException.class, () -> Webhooks.verify(body, header, secrets, at));
      assertTrue(e.getMessage().contains("empty or missing"));
    }
    assertEquals("evt_1", Webhooks.verify(body, header, List.of("whsec_synthetic_real"), at).id());
  }

  private static String reason(byte[] body, String header, List<String> secrets, java.time.Instant at) {
    return assertThrows(Webhooks.WebhookException.class, () -> Webhooks.verify(body, header, secrets, at)).reason();
  }
}
