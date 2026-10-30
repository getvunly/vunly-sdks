package com.vunly;

import com.vunly.internal.Json;
import java.net.URI;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;
import java.nio.charset.StandardCharsets;
import java.time.Duration;
import java.util.LinkedHashMap;
import java.util.Map;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.ExecutionException;
import java.util.concurrent.ThreadLocalRandom;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.TimeoutException;
import java.util.function.Function;
import java.util.regex.Matcher;
import java.util.regex.Pattern;

/** HTTP, retries by operation class, and error mapping. conformance/README.md. */
final class Transport {
  /** The most of one response held in memory (as the Go and .NET SDKs): the API's largest page is far below it. */
  static final int MAX_RESPONSE_BYTES = 16 * 1024 * 1024;

  /** Stands for "the body went over the cap and reading stopped" (compared by identity). */
  private static final byte[] TOO_LARGE = new byte[0];

  /**
   * Reads the body into memory but never more than {@link #MAX_RESPONSE_BYTES}: past it the subscription is cancelled (the JDK
   * closes the connection) and the result is {@link #TOO_LARGE}. A server that never stops sending costs the cap plus one chunk.
   */
  private static HttpResponse.BodyHandler<byte[]> cappedBody() {
    return info ->
        new HttpResponse.BodySubscriber<byte[]>() {
          private final CompletableFuture<byte[]> result = new CompletableFuture<>();
          private final java.io.ByteArrayOutputStream out = new java.io.ByteArrayOutputStream();
          private java.util.concurrent.Flow.Subscription subscription;
          private boolean done;

          @Override
          public void onSubscribe(java.util.concurrent.Flow.Subscription s) {
            subscription = s;
            s.request(1);
          }

          @Override
          public void onNext(java.util.List<java.nio.ByteBuffer> items) {
            if (done) {
              return;
            }
            for (java.nio.ByteBuffer b : items) {
              if (out.size() + (long) b.remaining() > MAX_RESPONSE_BYTES) {
                done = true;
                subscription.cancel();
                result.complete(TOO_LARGE);
                return;
              }
              byte[] chunk = new byte[b.remaining()];
              b.get(chunk);
              out.write(chunk, 0, chunk.length);
            }
            subscription.request(1);
          }

          @Override
          public void onError(Throwable t) {
            if (!done) {
              done = true;
              result.completeExceptionally(t);
            }
          }

          @Override
          public void onComplete() {
            if (!done) {
              done = true;
              result.complete(out.toByteArray());
            }
          }

          @Override
          public java.util.concurrent.CompletionStage<byte[]> getBody() {
            return result;
          }
        };
  }

  /** The operation class of the contract. */
  enum OpClass {
    /** Reads and the entitlement check. */
    R,
    /** Keyed mutations (the same id on a retry). */
    K,
    /** Naturally idempotent writes: upserts, deletes, subscribing to the same plan. */
    N,
    /** Everything else: never repeated after an ambiguous failure, only after a 429. */
    X
  }

  /** Waits between attempts. Replaced in tests. */
  interface Sleeper {
    void sleep(Duration d) throws InterruptedException;
  }

  record Request(
      String method, String path, Map<String, String> query, Object body, String idem, OpClass cls) {
    static Request get(String path, Map<String, String> query) {
      return new Request("GET", path, query, null, null, OpClass.R);
    }
  }

  record Meta(int status, String requestId, boolean replayed, boolean planOverage) {}

  record Result<T>(T value, Meta meta) {}

  private static final Duration MAX_RETRY_AFTER = Duration.ofSeconds(30);
  private static final Pattern ID = Pattern.compile("^[A-Za-z0-9_.:-]{8,128}$");

  private final HttpClient http;
  private final String baseUrl;
  private final String apiKey;
  private final Duration timeout;
  private final int maxRetries;
  private final Sleeper sleeper;
  private final String userAgent;

  Transport(
      HttpClient http,
      String baseUrl,
      String apiKey,
      Duration timeout,
      int maxRetries,
      Sleeper sleeper,
      String userAgent) {
    this.http = http;
    this.baseUrl = baseUrl;
    this.apiKey = apiKey;
    this.timeout = timeout;
    this.maxRetries = maxRetries;
    this.sleeper = sleeper;
    this.userAgent = userAgent;
  }

  /** Calls, retrying as the class allows, and parses a 2xx body with {@code parse} (null: the body is discarded). */
  <T> Result<T> call(Request r, Function<Fields, T> parse) {
    // A keyed mutation (class K) cannot run without its id: with none it would go out with no Idempotency-Key and be repeated
    // after a lost answer, which is exactly what the key exists to prevent. Refused here, before any request.
    if (r.cls() == OpClass.K && r.idem() == null) {
      throw invalidId("id is required: it names this one unit of work and is sent as the Idempotency-Key");
    }
    if (r.idem() != null && !ID.matcher(r.idem()).matches()) {
      throw invalidId("id must be 8 to 128 characters of A-Za-z0-9_.:- (it is sent as the Idempotency-Key)");
    }
    byte[] payload = r.body() == null ? null : Json.write(r.body()).getBytes(StandardCharsets.UTF_8);
    String target = baseUrl + r.path() + queryString(r.query());
    for (int attempt = 1; ; attempt++) {
      try {
        return once(r, target, payload, parse);
      } catch (UnavailableException e) {
        if ("cancelled".equals(e.code())
            || attempt > maxRetries
            || !mayRetry(r.cls(), e)) {
          throw e;
        }
        Duration wait = retryDelay(attempt, e.retryAfter());
        if (wait == null) {
          throw e;
        }
        try {
          sleeper.sleep(wait);
        } catch (InterruptedException ie) {
          Thread.currentThread().interrupt();
          throw cancelled();
        }
      }
    }
  }

  private static VunlyException invalidId(String message) {
    return VunlyException.of(
        400,
        "validation_error",
        message,
        null,
        Map.of("fields", Map.of("id", "8-128 chars of A-Za-z0-9_.:-")),
        null);
  }

  private static UnavailableException cancelled() {
    return new UnavailableException(
        "cancelled", 0, "the calling thread was interrupted", null, null, null);
  }

  /**
   * R, K and N are repeated after a lost answer, a timeout or a 5xx; X only after a 429 (refused before it ran). The
   * plan-volume stop is never repeated.
   */
  private static boolean mayRetry(OpClass cls, VunlyException e) {
    int s = e.status();
    if (!(s == 0 || s == 429 || s == 500 || s == 502 || s == 503 || s == 504)) {
      return false; // invalid_response and anything else
    }
    if (e instanceof UnavailableException u && u.isPlanStop()) {
      return false;
    }
    return !(cls == OpClass.X && s != 429);
  }

  /** Null when the server asked for a wait longer than we will wait: the caller decides. */
  static Duration retryDelay(int attempt, Duration retryAfter) {
    if (retryAfter != null && !retryAfter.isZero()) {
      return retryAfter.compareTo(MAX_RETRY_AFTER) <= 0 ? retryAfter : null;
    }
    long baseMs = Math.min(8000L, 250L << (attempt - 1));
    return Duration.ofMillis((long) (baseMs * (0.5 + ThreadLocalRandom.current().nextDouble() / 2)));
  }

  private <T> Result<T> once(Request r, String target, byte[] payload, Function<Fields, T> parse) {
    HttpRequest request;
    try {
      request = buildRequest(r, target, payload);
    } catch (IllegalArgumentException e) {
      // The JDK's message can quote the offending header value, which here may be the key: never pass it on.
      throw new IllegalStateException(
          "vunly: the request could not be built (the address or a header holds a character that cannot be sent)");
    }
    return send(request, parse);
  }

  private HttpRequest buildRequest(Request r, String target, byte[] payload) {
    HttpRequest.Builder b =
        HttpRequest.newBuilder(URI.create(target))
            .timeout(timeout)
            .header("Authorization", "Bearer " + apiKey)
            .header("User-Agent", userAgent)
            .header("Accept", "application/json");
    if (target.startsWith("http://")) {
      // The JDK client otherwise sends "Connection: Upgrade, HTTP2-Settings" and "Upgrade: h2c" on every cleartext request,
      // and many servers (Node's http server among them) drop such a connection. Cleartext is the local sandbox address;
      // https keeps negotiating HTTP/2 through ALPN.
      b.version(HttpClient.Version.HTTP_1_1);
    }
    if (payload != null) {
      b.header("Content-Type", "application/json");
    }
    if (r.idem() != null) {
      b.header("Idempotency-Key", r.idem());
    }
    b.method(
        r.method(),
        payload == null
            ? HttpRequest.BodyPublishers.noBody()
            : HttpRequest.BodyPublishers.ofByteArray(payload));
    return b.build();
  }

  private <T> Result<T> send(HttpRequest request, Function<Fields, T> parse) {
    CompletableFuture<HttpResponse<byte[]>> future = http.sendAsync(request, cappedBody());
    HttpResponse<byte[]> res;
    try {
      // The request timeout covers the answer's headers; get() bounds the whole attempt, body included.
      res = future.get(timeout.toMillis(), TimeUnit.MILLISECONDS);
    } catch (InterruptedException e) {
      future.cancel(true);
      Thread.currentThread().interrupt();
      throw cancelled();
    } catch (TimeoutException e) {
      future.cancel(true);
      throw unavailable("the Vunly API did not answer within " + timeout);
    } catch (ExecutionException | RuntimeException e) {
      // Deliberately drop the underlying exception: some transports put the URL or headers in the message.
      throw unavailable(
          e.getCause() instanceof java.net.http.HttpTimeoutException
              ? "the Vunly API did not answer within " + timeout
              : "could not reach the Vunly API");
    }

    int status = res.statusCode();
    String requestId = sanitizeId(res.headers().firstValue("request-id").orElse(null));
    if (status >= 300 && status < 400) {
      // Never followed (the key must not travel to another host) and never retried, whatever the body says: the body of a
      // redirect is not an API answer, so its code is not trusted either.
      throw new UnavailableException(
          "unexpected_redirect",
          status,
          "the API answered a redirect (" + status + "), which the SDK never follows: check the API address you configured",
          requestId,
          null,
          null);
    }
    Meta meta =
        new Meta(
            status,
            requestId,
            res.headers().firstValue("idempotent-replayed").map("true"::equals).orElse(false),
            res.headers().firstValue("vunly-plan-overage").map("true"::equals).orElse(false));
    byte[] raw = res.body();
    if (raw == TOO_LARGE) {
      // A 2xx is the SDK's own error and is not retried (the call may have been applied, so retrying later with the SAME id is
      // safe); an error status is classified by its status, as an error with no body is.
      if (status >= 200 && status < 300) {
        throw VunlyException.invalidResponse(
            status, requestId, "the response is larger than " + (MAX_RESPONSE_BYTES >> 20) + " MB");
      }
      raw = new byte[0];
    }
    if (status >= 200 && status < 300) {
      if (parse == null) {
        return new Result<>(null, meta);
      }
      try {
        return new Result<>(parse.apply(Fields.of(Json.parse(raw))), meta);
      } catch (Json.JsonException e) {
        throw VunlyException.invalidResponse(status, requestId, "the body is not JSON");
      } catch (RuntimeException e) {
        // Why text() here is belt and braces, not a mutation-testable guard: a Json.JsonException (the only parser failure, whose
        // message can echo a character of the body) is caught above with a fixed message. What reaches this branch comes from
        // Fields, whose messages are built only from the SDK's own field names and array indexes ("field \"createdAt\" is not
        // ..."); the one place that parses server text, Fields.instant, catches DateTimeParseException (whose message quotes
        // the text) and throws bad() instead. HardeningTest pins that a hostile value is never echoed. text() stays so a
        // future message that did carry server text still comes out cleaned, bounded to 200 and key-redacted.
        throw VunlyException.invalidResponse(status, requestId, text(e.getMessage(), 200));
      }
    }
    throw errorFromResponse(res, requestId, raw);
  }

  /** A server's text made safe for a message or a log line: no control, BiDi or zero-width characters, key redacted, bounded. */
  private String text(String s, int max) {
    return clean(s, max, apiKey);
  }

  private String sanitizeId(String s) {
    String c = text(s, MAX_REQUEST_ID);
    return c == null || c.isEmpty() ? null : c;
  }

  private static final int MAX_CODE = 64;
  private static final int MAX_MESSAGE = 500;
  private static final int MAX_REQUEST_ID = 128;

  /** A copy of the API's details that is safe to keep and print: bounded depth and width, every string cleaned. */
  private Object cleanDetails(Object v, int depth) {
    if (v instanceof String s) {
      return text(s, MAX_MESSAGE);
    }
    if (v instanceof Map<?, ?> m) {
      if (depth > 4) {
        return null;
      }
      Map<String, Object> out = new LinkedHashMap<>();
      int n = 0;
      for (Map.Entry<?, ?> e : m.entrySet()) {
        if (n++ >= 50) {
          break;
        }
        String k = e.getKey() instanceof String ks ? text(ks, MAX_CODE) : null;
        Object item = cleanDetails(e.getValue(), depth + 1);
        if (k != null && item != null) {
          out.put(k, item);
        }
      }
      return out;
    }
    if (v instanceof java.util.List<?> l) {
      if (depth > 4) {
        return null;
      }
      java.util.List<Object> out = new java.util.ArrayList<>();
      for (Object item : l) {
        if (out.size() >= 50) {
          break;
        }
        out.add(cleanDetails(item, depth + 1));
      }
      return out;
    }
    return v;
  }

  private static UnavailableException unavailable(String message) {
    return new UnavailableException("unavailable", 0, message, null, null, null);
  }

  @SuppressWarnings("unchecked")
  private VunlyException errorFromResponse(HttpResponse<byte[]> res, String requestId, byte[] raw) {
    int status = res.statusCode();
    String code = null;
    String message = null;
    Map<String, Object> details = null;
    try {
      Object parsed = Json.parse(raw);
      if (parsed instanceof Map<?, ?> top && top.get("error") instanceof Map<?, ?> err) {
        code = err.get("code") instanceof String s ? text(s, MAX_CODE) : null;
        message = err.get("message") instanceof String s ? s : null;
        if (err.get("requestId") instanceof String s && sanitizeId(s) != null) {
          requestId = sanitizeId(s);
        }
        if (err.get("details") instanceof Map<?, ?> d) {
          details = (Map<String, Object>) cleanDetails(d, 0);
        }
      }
    } catch (RuntimeException ignored) {
      // an error with no JSON body (a proxy, an empty 5xx) is still classified by its status
    }
    if (code == null || code.isEmpty()) {
      code = VunlyException.fallbackCode(status);
    }
    if (message == null || message.isEmpty()) {
      message = "request failed with status " + status;
    }
    if (status == 404 && message.toLowerCase(java.util.Locale.ROOT).contains("no such route")) {
      message +=
          " (check the API address you configured: it should be the address only, for example https://host;"
              + " a trailing /v1 is accepted)";
    }
    Duration retryAfter = null;
    String ra = res.headers().firstValue("retry-after").orElse(null);
    if (ra != null) {
      retryAfter = parseRetryAfter(ra);
    }
    // What the server sent is data, not text to trust: control characters (a newline, an escape sequence) would let it forge
    // log lines, and an enormous value would fill them.
    return VunlyException.of(status, code, text(message, MAX_MESSAGE), requestId, details, retryAfter);
  }

  /**
   * The whole syntax of a Retry-After this SDK honours (conformance/README.md): ASCII digits, an optional
   * fraction, an optional exponent, an optional leading plus, or Inf / Infinity. Double.parseDouble alone would also accept
   * "0x1p3", "1d" and "NaN".
   */
  private static final Pattern RETRY_AFTER =
      Pattern.compile("(?i)^\\s*(\\+?(?:[0-9]+(?:\\.[0-9]*)?|\\.[0-9]+)(?:e[+-]?[0-9]+)?|\\+?inf(?:inity)?)\\s*$");

  /**
   * The wait a Retry-After asks for, or null when it is not a number (an HTTP date, garbage: the normal backoff applies). A number
   * too large to represent ("1e999", hundreds of digits, Inf) is a very long wait, NOT an absent header: it is capped at one day
   * and the caller surfaces it at once instead of retrying early.
   */
  static Duration parseRetryAfter(String header) {
    if (header == null) {
      return null;
    }
    Matcher m = RETRY_AFTER.matcher(header);
    if (!m.matches()) {
      return null;
    }
    String number = m.group(1);
    // Double.parseDouble returns Infinity on overflow instead of throwing; "inf" itself it would not parse
    double secs =
        number.toLowerCase(java.util.Locale.ROOT).contains("inf")
            ? Double.POSITIVE_INFINITY
            : Double.parseDouble(number);
    return Duration.ofMillis((long) (Math.min(secs, MAX_REPORTED_RETRY_AFTER_SECONDS) * 1000));
  }
  /** The largest wait reported to the caller; the SDK itself never waits more than {@link #MAX_RETRY_AFTER}. */
  private static final double MAX_REPORTED_RETRY_AFTER_SECONDS = 86_400;

  /**
   * Strips control, format (BiDi marks and overrides, zero-width, BOM) and line/paragraph separator characters, replaces an
   * unpaired surrogate, redacts every occurrence of {@code key} (after the stripping, so a key split by zero-width characters
   * is found too) and bounds the length to {@code max} characters; null stays null.
   */
  static String clean(String s, int max, String key) {
    if (s == null) {
      return null;
    }
    StringBuilder sb = new StringBuilder(s.length());
    for (int i = 0; i < s.length(); ) {
      int cp = s.codePointAt(i);
      i += Character.charCount(cp);
      int type = Character.getType(cp);
      if (type == Character.CONTROL
          || type == Character.FORMAT
          || type == Character.LINE_SEPARATOR
          || type == Character.PARAGRAPH_SEPARATOR) {
        continue;
      }
      sb.appendCodePoint(type == Character.SURROGATE ? 0xFFFD : cp);
    }
    String out = sb.toString();
    if (key != null && !key.isEmpty()) {
      out = out.replace(key, "[redacted]");
    }
    if (out.length() > max) {
      int end = max;
      if (Character.isHighSurrogate(out.charAt(end - 1))) {
        end--;
      }
      out = out.substring(0, end);
    }
    return out;
  }

  private static String queryString(Map<String, String> query) {
    if (query == null || query.isEmpty()) {
      return "";
    }
    StringBuilder sb = new StringBuilder("?");
    boolean first = true;
    for (Map.Entry<String, String> e : query.entrySet()) {
      if (!first) {
        sb.append('&');
      }
      first = false;
      sb.append(java.net.URLEncoder.encode(e.getKey(), StandardCharsets.UTF_8))
          .append('=')
          .append(java.net.URLEncoder.encode(e.getValue(), StandardCharsets.UTF_8));
    }
    return sb.toString();
  }

  /** Percent-encodes one path segment (everything but unreserved characters), so a customer id cannot change the route. */
  static String segment(String s) {
    // "", "." and ".." would change the route on the wire (GET /v1/customers/ , GET /v1/): refused before any request
    if (s == null || s.isEmpty() || s.equals(".") || s.equals("..")) {
      throw VunlyException.of(
          400,
          "validation_error",
          "an id in the path must not be empty, \".\" or \"..\"",
          null,
          Map.of("fields", Map.of("id", "not empty, \".\" or \"..\"")),
          null);
    }
    StringBuilder sb = new StringBuilder();
    for (byte b : s.getBytes(StandardCharsets.UTF_8)) {
      int c = b & 0xff;
      if ((c >= 'A' && c <= 'Z') || (c >= 'a' && c <= 'z') || (c >= '0' && c <= '9')
          || c == '-' || c == '.' || c == '_' || c == '~') {
        sb.append((char) c);
      } else {
        sb.append('%').append(String.format("%02X", c));
      }
    }
    return sb.toString();
  }

  static Map<String, String> query(ListOptions o) {
    Map<String, String> q = new LinkedHashMap<>();
    if (o != null) {
      if (o.limit() != 0) {
        q.put("limit", Integer.toString(o.limit()));
      }
      if (o.cursor() != null && !o.cursor().isEmpty()) {
        q.put("cursor", o.cursor());
      }
    }
    return q;
  }
}
