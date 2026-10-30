package vunly

import (
	"errors"
	"fmt"
	"sort"
	"strconv"
	"strings"
	"time"
	"unicode"
)

// APIError is what every failure carries. Match on the four typed errors below (errors.As) or on Code, never on Message.
//
// The message never contains your key.
type APIError struct {
	// Code is the API's stable machine code ("insufficient_credits", "rate_limited", ...) or one of the SDK's own:
	// "unavailable" (no answer) and "invalid_response" (a 2xx whose body lacks a promised field).
	Code string
	// Status is the HTTP status, or 0 when there was no response.
	Status int
	// Message is for people and may change.
	Message string
	// RequestID is the API's Request-Id. Quote it in a support request.
	RequestID string
	// Details is the API's structured detail, if any (for example balance and required on a 402).
	Details map[string]any
	// RetryAfter is the server's Retry-After, when it sent one.
	RetryAfter time.Duration

	// cause is the error underneath, when there is one a caller may want to test with errors.Is: today the caller's
	// expired context deadline (context.DeadlineExceeded).
	cause error
}

// Unwrap exposes the cause, so errors.Is(err, context.DeadlineExceeded) works through the typed wrappers.
func (e *APIError) Unwrap() error { return e.cause }

func (e *APIError) Error() string {
	s := "vunly: " + e.Message
	if e.Status != 0 {
		s += " (" + strconv.Itoa(e.Status) + " " + e.Code + ")"
	} else {
		s += " (" + e.Code + ")"
	}
	if e.RequestID != "" {
		s += " request " + e.RequestID
	}
	return s
}

// AuthError is a 401 or 403: a missing, wrong or revoked key, or a key without the scope. Not retryable.
type AuthError struct{ *APIError }

// LimitReachedError is a 402: the customer has too few credits. Nothing was debited. Do not run the work. Not retryable.
type LimitReachedError struct{ *APIError }

// InvalidRequestError is every other 4xx (400, 404, 409) and any local validation failure: fix the request.
type InvalidRequestError struct{ *APIError }

// UnavailableError is no answer (network, timeout), a 429, a 5xx that survived the SDK's own retries, an
// "invalid_response", or a 3xx ("unexpected_redirect": never followed, never retried). It is safe to try again later with the SAME id. A 429 whose Details["reason"] is
// "plan_included_volume" is the Free plan's monthly volume stop, not a rate limit: it is never retried.
type UnavailableError struct{ *APIError }

func (e *AuthError) Unwrap() error           { return e.APIError }
func (e *LimitReachedError) Unwrap() error   { return e.APIError }
func (e *InvalidRequestError) Unwrap() error { return e.APIError }
func (e *UnavailableError) Unwrap() error    { return e.APIError }

// Balance is the customer's balance reported by a 402, when the API sent it.
func (e *LimitReachedError) Balance() (int64, bool) { return detailInt(e.Details, "balance") }

// Required is the cost the refused request needed, when the API sent it.
func (e *LimitReachedError) Required() (int64, bool) { return detailInt(e.Details, "required") }

// PlanStop reports whether this is the Free plan's monthly volume stop (never retried; Retry-After is the time to the
// start of the next month).
func (e *UnavailableError) PlanStop() bool { return isPlanStop(e.APIError) }

func detailInt(d map[string]any, key string) (int64, bool) {
	switch v := d[key].(type) {
	case float64:
		return int64(v), true
	case int64:
		return v, true
	}
	return 0, false
}

func isPlanStop(e *APIError) bool {
	return e != nil && e.Status == 429 && e.Details != nil && e.Details["reason"] == "plan_included_volume"
}

// IsAuth, IsLimitReached, IsInvalidRequest and IsUnavailable report the class of an error without a type assertion.
func IsAuth(err error) bool { var e *AuthError; return errors.As(err, &e) }

// IsLimitReached reports a 402: do not run the work.
func IsLimitReached(err error) bool { var e *LimitReachedError; return errors.As(err, &e) }

// IsInvalidRequest reports a request the API will not accept as sent.
func IsInvalidRequest(err error) bool { var e *InvalidRequestError; return errors.As(err, &e) }

// IsUnavailable reports an unknown or temporary outcome: try again later with the same id, or fail closed.
func IsUnavailable(err error) bool { var e *UnavailableError; return errors.As(err, &e) }

func newError(status int, code, message, requestID string, details map[string]any, retryAfter time.Duration) error {
	base := &APIError{Code: code, Status: status, Message: message, RequestID: requestID, Details: details, RetryAfter: retryAfter}
	switch {
	case status == 402:
		return &LimitReachedError{base}
	case status == 401 || status == 403:
		return &AuthError{base}
	case status == 429 || status >= 500 || status == 0 || (status >= 300 && status < 400):
		// A 3xx (unexpected_redirect) is Unavailable like in every other SDK; mayRetry never repeats it.
		return &UnavailableError{base}
	default:
		return &InvalidRequestError{base}
	}
}

// fallbackCode is the code the API would send for a status when the error body is missing (a proxy, an empty 5xx).
func fallbackCode(status int) string {
	switch {
	case status == 401 || status == 403:
		return "unauthenticated"
	case status == 402:
		return "insufficient_credits"
	case status == 429:
		return "rate_limited"
	case status >= 500:
		return "internal_error"
	case status >= 300 && status < 400:
		return "unexpected_redirect" // the SDK never follows one: the base URL is probably wrong
	}
	return "validation_error"
}

// Bounds on the server's text (the same as the other SDKs).
const (
	maxMessage   = 500
	maxCode      = 64
	maxRequestID = 128
	maxDetails   = 50 // entries per object or array
	maxDepth     = 4
)

// cleanText makes text the server supplied safe to print: valid UTF-8, no control, BiDi or zero-width characters
// (categories Cc, Cf, Zl, Zp), the client's own key replaced by "[redacted]" (a server may echo the Authorization
// header), and at most max characters. The key is removed after the invisible characters, so a key split by a zero-width
// character is still found, and before the bound, so a cut never leaves a piece of the key.
func cleanText(s string, max int, key string) string {
	s = strings.ToValidUTF8(s, "?")
	s = strings.Map(func(r rune) rune {
		if unicode.In(r, unicode.Cc, unicode.Cf, unicode.Zl, unicode.Zp) {
			return -1
		}
		return r
	}, s)
	if key != "" {
		s = strings.ReplaceAll(s, key, "[redacted]")
	}
	s = strings.TrimSpace(s)
	if r := []rune(s); len(r) > max {
		s = string(r[:max])
	}
	return s
}

// cleanDetails returns a copy of the API's details that is safe to keep: at most 5 levels and 50 entries per object or
// array, keys cleaned to 64 characters and strings to 500, nothing but JSON scalars.
func cleanDetails(v any, depth int, key string) any {
	switch x := v.(type) {
	case map[string]any:
		if depth > maxDepth {
			return nil
		}
		out := make(map[string]any, len(x))
		names := make([]string, 0, len(x))
		for k := range x {
			names = append(names, k)
		}
		sort.Strings(names)
		for _, k := range names {
			if len(out) >= maxDetails {
				break
			}
			if name := cleanText(k, maxCode, key); name != "" {
				out[name] = cleanDetails(x[k], depth+1, key)
			}
		}
		return out
	case []any:
		if depth > maxDepth {
			return nil
		}
		if len(x) > maxDetails {
			x = x[:maxDetails]
		}
		out := make([]any, len(x))
		for i, item := range x {
			out[i] = cleanDetails(item, depth+1, key)
		}
		return out
	case string:
		return cleanText(x, maxMessage, key)
	case float64, bool, nil:
		return x
	}
	return nil
}

func invalidResponse(status int, requestID, what string) error {
	// Unavailable whatever the status: the call may have been applied, so retrying later with the SAME id is safe.
	return &UnavailableError{&APIError{
		Code:      "invalid_response",
		Status:    status,
		Message:   fmt.Sprintf("the API answered %d but the response is malformed (%s)", status, what),
		RequestID: requestID,
	}}
}
