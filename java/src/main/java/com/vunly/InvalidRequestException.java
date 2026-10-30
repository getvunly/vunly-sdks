package com.vunly;

import java.time.Duration;
import java.util.Map;

/** Every other 4xx (400, 404, 409) and any local validation failure: fix the request. Not retryable. */
public final class InvalidRequestException extends VunlyException {
  private static final long serialVersionUID = 1L;

  InvalidRequestException(
      String code, int status, String message, String requestId, Map<String, Object> details, Duration retryAfter) {
    super(code, status, message, requestId, details, retryAfter);
  }
}
