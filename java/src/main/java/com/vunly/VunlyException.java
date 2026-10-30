package com.vunly;

import java.time.Duration;
import java.util.Map;

/**
 * Every failure of a Vunly call. Unchecked on purpose: a billing call that fails is almost never something the
 * caller can repair on the spot, and checked exceptions would leak through every lambda and framework callback. Catch
 * the four subclasses that matter ({@link AuthException}, {@link LimitReachedException},
 * {@link InvalidRequestException}, {@link UnavailableException}) or this class; match on {@link #code()}, never on the
 * message.
 *
 * <p>The message never contains your key.
 */
public class VunlyException extends RuntimeException {
  private static final long serialVersionUID = 1L;

  private final String code;
  private final int status;
  private final String requestId;
  private final transient Map<String, Object> details;
  private final transient Duration retryAfter;

  VunlyException(
      String code,
      int status,
      String message,
      String requestId,
      Map<String, Object> details,
      Duration retryAfter) {
    super(message);
    this.code = code;
    this.status = status;
    this.requestId = requestId;
    this.details = details == null ? Map.of() : Map.copyOf(details);
    this.retryAfter = retryAfter;
  }

  /**
   * The API's stable machine code ("insufficient_credits", "rate_limited", ...) or one of the SDK's own:
   * "unavailable" (no answer) and "invalid_response" (a 2xx whose body lacks a promised field).
   */
  public String code() {
    return code;
  }

  /** The HTTP status, or 0 when there was no response. */
  public int status() {
    return status;
  }

  /** The API's {@code Request-Id}: quote it in a support request. May be null. */
  public String requestId() {
    return requestId;
  }

  /** The API's structured detail (for example balance and required on a 402). Never null. */
  public Map<String, Object> details() {
    return details;
  }

  /** The server's {@code Retry-After}, or null when it sent none. */
  public Duration retryAfter() {
    return retryAfter;
  }

  @Override
  public String getMessage() {
    StringBuilder sb = new StringBuilder("vunly: ").append(super.getMessage());
    sb.append(" (");
    if (status != 0) {
      sb.append(status).append(' ');
    }
    sb.append(code).append(')');
    if (requestId != null && !requestId.isEmpty()) {
      sb.append(" request ").append(requestId);
    }
    return sb.toString();
  }

  static VunlyException of(
      int status,
      String code,
      String message,
      String requestId,
      Map<String, Object> details,
      Duration retryAfter) {
    if (status == 402) {
      return new LimitReachedException(code, status, message, requestId, details, retryAfter);
    }
    if (status == 401 || status == 403) {
      return new AuthException(code, status, message, requestId, details, retryAfter);
    }
    // A 3xx is never followed (the key must not travel to another host) and is not the caller's invalid request: it means the
    // address is wrong or a proxy answered. Unavailable, not retried (the status is not a retryable one).
    if (status == 0 || status == 429 || status >= 500 || (status >= 300 && status < 400)) {
      return new UnavailableException(code, status, message, requestId, details, retryAfter);
    }
    return new InvalidRequestException(code, status, message, requestId, details, retryAfter);
  }

  /** The code the API would send for a status when the error body is missing (a proxy, an empty 5xx). */
  static String fallbackCode(int status) {
    if (status == 401 || status == 403) {
      return "unauthenticated";
    }
    if (status == 402) {
      return "insufficient_credits";
    }
    if (status == 429) {
      return "rate_limited";
    }
    if (status >= 500) {
      return "internal_error";
    }
    if (status >= 300 && status < 400) {
      return "unexpected_redirect";
    }
    return "validation_error";
  }

  /** A 2xx whose body lacks a promised field: unknown outcome, so Unavailable whatever the status. */
  static UnavailableException invalidResponse(int status, String requestId, String what) {
    return new UnavailableException(
        "invalid_response",
        status,
        "the API answered " + status + " but the response is malformed (" + what + ")",
        requestId,
        null,
        null);
  }
}
