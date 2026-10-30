package example;

import com.vunly.Entitlement;
import com.vunly.Estimate;
import com.vunly.InvalidRequestException;
import com.vunly.LimitReachedException;
import com.vunly.UnavailableException;
import com.vunly.UsageResult;
import com.vunly.Vunly;
import java.util.Map;
import java.util.UUID;
import org.springframework.beans.factory.annotation.Value;
import org.springframework.http.HttpStatus;
import org.springframework.http.ResponseEntity;
import org.springframework.web.bind.annotation.ExceptionHandler;
import org.springframework.web.bind.annotation.PostMapping;
import org.springframework.web.bind.annotation.RequestBody;
import org.springframework.web.bind.annotation.RequestHeader;
import org.springframework.web.bind.annotation.RestController;
import org.springframework.web.server.ResponseStatusException;

/** Ask, do the work, record it once. */
@RestController
class GenerateController {
  private static final String METER = "tokens";
  private static final String FEATURE = "generate";

  private final Vunly vunly;
  private final boolean trustRequestId;

  /**
   * {@code vunly.trust-request-id} (default false): use the caller's {@code X-Request-Id} as the idempotency id. OFF by default,
   * because a replay of an id does not charge again: a caller that re-sends the same header gets the work done again for free.
   * Turn it on only when the id is one YOU issue and bind to the customer (for example a retry token your own gateway
   * generates), never one a client can choose.
   */
  GenerateController(Vunly vunly, @Value("${vunly.trust-request-id:false}") boolean trustRequestId) {
    this.vunly = vunly;
    this.trustRequestId = trustRequestId;
  }

  record GenerateRequest(String customer, String prompt) {}

  @PostMapping("/generate")
  Map<String, Object> generate(
      @RequestBody GenerateRequest req,
      @RequestHeader(value = "X-Request-Id", required = false) String requestId) {
    if (req == null || req.customer() == null || req.customer().isBlank() || req.prompt() == null || req.prompt().isBlank()) {
      throw new ResponseStatusException(HttpStatus.BAD_REQUEST, "customer and prompt are required");
    }
    // The id names this unit of work: one id, one charge. By default the server issues it; see the constructor.
    String id =
        trustRequestId && requestId != null
            ? requestId
            : "req_" + UUID.randomUUID().toString().replace("-", "");
    long tokens = Math.max(1, req.prompt().length() * 10L);

    // 1. Ask first. A "no", and an error, both mean: do not run the work (the exception handler below answers 503).
    Entitlement decision = vunly.entitlements().check(req.customer(), FEATURE, new Estimate(METER, tokens));
    if (!decision.allowed()) {
      throw new ResponseStatusException(HttpStatus.FORBIDDEN, "not allowed: " + decision.reason());
    }

    // 2. The work. A real application calls its model here.
    String output = "echo: " + req.prompt();

    // 3. Record it once. A LimitReachedException means the credits were not there when it counted (the check is only
    // advisory); nothing was debited.
    UsageResult usage = vunly.usage().record(id, req.customer(), METER, tokens);
    return Map.of(
        "output", output,
        "id", id,
        "creditsRemaining", usage.remaining(),
        "replayed", usage.replayed());
  }

  @ExceptionHandler(LimitReachedException.class)
  @org.springframework.web.bind.annotation.ResponseStatus(HttpStatus.PAYMENT_REQUIRED)
  Map<String, Object> limitReached(LimitReachedException e) {
    return Map.of("error", "insufficient_credits", "requestId", String.valueOf(e.requestId()));
  }

  /**
   * The caller's mistake, not ours: a malformed id (the X-Request-Id header becomes the idempotency id), an unknown customer,
   * a rejected request. The SDK carries the 4xx the API chose; answer with it instead of letting it turn into a 500.
   */
  @ExceptionHandler(InvalidRequestException.class)
  ResponseEntity<Map<String, Object>> invalid(InvalidRequestException e) {
    int status = e.status() >= 400 && e.status() < 500 ? e.status() : 400;
    return ResponseEntity.status(status)
        .body(Map.of("error", e.code(), "requestId", String.valueOf(e.requestId())));
  }

  /** Fail closed: when Vunly cannot be reached the work is not done, and the caller may retry with the same id. */
  @ExceptionHandler(UnavailableException.class)
  @org.springframework.web.bind.annotation.ResponseStatus(HttpStatus.SERVICE_UNAVAILABLE)
  Map<String, Object> unavailable(UnavailableException e) {
    return Map.of("error", "billing_unavailable", "code", e.code());
  }
}
