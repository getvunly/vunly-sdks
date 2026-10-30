package com.vunly;

import java.time.Duration;
import java.util.Map;

/** 401 or 403: a missing, wrong or revoked key, or a key without the scope. Not retryable. */
public final class AuthException extends VunlyException {
  private static final long serialVersionUID = 1L;

  AuthException(
      String code, int status, String message, String requestId, Map<String, Object> details, Duration retryAfter) {
    super(code, status, message, requestId, details, retryAfter);
  }
}
