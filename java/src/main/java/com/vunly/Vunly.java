package com.vunly;

import java.net.URI;
import java.net.http.HttpClient;
import java.time.Duration;
import java.util.Objects;
import java.util.regex.Pattern;

/**
 * The Vunly client. Immutable and safe for concurrent use: create it once and share it (a Spring bean, a static field).
 * Calls block the calling thread; they are cheap on virtual threads.
 *
 * <pre>{@code
 * Vunly vunly = Vunly.builder()
 *     .apiKey(System.getenv("VUNLY_API_KEY"))
 *     .baseUrl(System.getenv("VUNLY_API_URL"))
 *     .build();
 * UsageResult r = vunly.usage().record("req_2026_0001", "cus_demo_01", "tokens", 1050);
 * }</pre>
 *
 * <p>Neither the key nor the address is read from the environment or from disk by the SDK. No global state.
 */
public final class Vunly {
  /** The SDK version, also sent in the User-Agent. */
  public static final String VERSION = "0.1.0";

  private static final Pattern VERSION_SUFFIX = Pattern.compile("(?i)/v1$");
  /** Printable ASCII without a space: what can be a header value without surprises. */
  private static final Pattern KEY_CHARS = Pattern.compile("^[\\x21-\\x7E]+$");

  private final String baseUrl;
  private final CustomersApi customers;
  private final CreditsApi credits;
  private final UsageApi usage;
  private final EntitlementsApi entitlements;
  private final PlansApi plans;
  private final CustomerTokensApi customerTokens;
  private final Transport transport;

  private Vunly(Builder b) {
    this.baseUrl = b.baseUrl;
    this.transport =
        new Transport(
            b.httpClient,
            b.baseUrl,
            b.apiKey,
            b.timeout,
            b.maxRetries,
            b.sleeper,
            "vunly-java/"
                + VERSION
                + " (Java "
                + System.getProperty("java.version")
                + "; "
                + System.getProperty("os.name")
                + " "
                + System.getProperty("os.arch")
                + ")");
    this.customers = new CustomersApi(transport);
    this.credits = new CreditsApi(transport);
    this.usage = new UsageApi(transport);
    this.entitlements = new EntitlementsApi(transport);
    this.plans = new PlansApi(transport);
    this.customerTokens = new CustomerTokensApi(transport);
  }

  public static Builder builder() {
    return new Builder();
  }

  public CustomersApi customers() {
    return customers;
  }

  public CreditsApi credits() {
    return credits;
  }

  public UsageApi usage() {
    return usage;
  }

  public EntitlementsApi entitlements() {
    return entitlements;
  }

  public PlansApi plans() {
    return plans;
  }

  public CustomerTokensApi customerTokens() {
    return customerTokens;
  }

  /**
   * The project's plan and this month's recorded usage. Its limits are inside {@link PlanUsage#unconfirmedLimits()} and
   * are not a guarantee ({@code confirmed} is false).
   */
  public PlanUsage planUsage() {
    return transport
        .call(Transport.Request.get("/v1/plan-usage", null), PlanUsage::from)
        .value();
  }

  /** Never contains the key. */
  @Override
  public String toString() {
    return "Vunly{baseUrl=" + baseUrl + ", apiKey=<hidden>}";
  }

  /** The address without a trailing slash and without a trailing {@code /v1}: the SDK adds {@code /v1/...} itself. */
  public static String normalizeBaseUrl(String url) {
    String out = url.trim().replaceAll("/+$", "");
    out = VERSION_SUFFIX.matcher(out).replaceAll("");
    return out.replaceAll("/+$", "");
  }

  /** Configures a {@link Vunly}. {@code apiKey} and {@code baseUrl} are required. */
  public static final class Builder {
    private String apiKey;
    private String baseUrl;
    private Duration timeout = Duration.ofSeconds(10);
    private int maxRetries = 2;
    private HttpClient httpClient;
    private Transport.Sleeper sleeper = d -> Thread.sleep(d.toMillis());

    private Builder() {}

    /** The sandbox key (vk_test_...). Read it from your environment or secret store; never put it in source. */
    public Builder apiKey(String apiKey) {
      this.apiKey = apiKey;
      return this;
    }

    /** The API address from the setup step, for example {@code https://host}. A trailing slash or /v1 is accepted. */
    public Builder baseUrl(String baseUrl) {
      this.baseUrl = baseUrl;
      return this;
    }

    /** The limit for ONE attempt, answer included (default 10 s). */
    public Builder timeout(Duration timeout) {
      this.timeout = Objects.requireNonNull(timeout, "timeout");
      return this;
    }

    /** Retries after a lost answer, timeout, 429 or 5xx, by operation class (default 2; 0 turns retries off). */
    public Builder maxRetries(int maxRetries) {
      this.maxRetries = maxRetries;
      return this;
    }

    /**
     * Your own {@link HttpClient} (proxy, TLS settings, executor). It must not follow redirects: a redirect would carry
     * the Authorization header to another host. The default client never redirects.
     */
    public Builder httpClient(HttpClient httpClient) {
      this.httpClient = Objects.requireNonNull(httpClient, "httpClient");
      return this;
    }

    /** For tests of this package: replaces the wait between retries. */
    Builder sleeper(Transport.Sleeper sleeper) {
      this.sleeper = sleeper;
      return this;
    }

    public Vunly build() {
      if (apiKey == null || apiKey.isBlank()) {
        throw new IllegalArgumentException(
            "vunly: the API key is empty (read it from your environment)");
      }
      // A key read from a file or a secret mount often ends in a newline. Trim it, and refuse what still cannot be sent in a
      // header; the message names no part of the value (the JDK's own exception would quote all of it).
      apiKey = apiKey.strip();
      if (!KEY_CHARS.matcher(apiKey).matches()) {
        throw new IllegalArgumentException(
            "vunly: the API key holds a character that cannot be sent in a header (a space, a control character or a non-ASCII character); check the value you read it from");
      }
      if (baseUrl == null || baseUrl.isBlank()) {
        throw new IllegalArgumentException(
            "vunly: the base URL is empty (use baseUrl(...) with the API address from the setup step)");
      }
      if (maxRetries < 0) {
        throw new IllegalArgumentException("vunly: maxRetries must not be negative");
      }
      if (timeout.isZero() || timeout.isNegative()) {
        throw new IllegalArgumentException("vunly: timeout must be positive");
      }
      baseUrl = normalizeBaseUrl(baseUrl);
      URI u;
      try {
        u = URI.create(baseUrl);
      } catch (IllegalArgumentException e) {
        throw new IllegalArgumentException(
            "vunly: the base URL must be an absolute http(s) address such as https://host");
      }
      if (u.getHost() == null || !("http".equals(u.getScheme()) || "https".equals(u.getScheme()))) {
        throw new IllegalArgumentException(
            "vunly: the base URL must be an absolute http(s) address such as https://host");
      }
      if (u.getUserInfo() != null || u.getRawQuery() != null || u.getRawFragment() != null) {
        throw new IllegalArgumentException(
            "vunly: the base URL must be the address only (no credentials, query or fragment)");
      }
      if (httpClient != null && httpClient.followRedirects() != HttpClient.Redirect.NEVER) {
        // A followed redirect would carry the Authorization and Idempotency-Key headers and the body to another host.
        throw new IllegalArgumentException(
            "vunly: the HttpClient follows redirects; build it with followRedirects(HttpClient.Redirect.NEVER)");
      }
      if (httpClient == null) {
        httpClient =
            HttpClient.newBuilder()
                .followRedirects(HttpClient.Redirect.NEVER)
                .connectTimeout(timeout)
                .build();
      }
      return new Vunly(this);
    }
  }
}
