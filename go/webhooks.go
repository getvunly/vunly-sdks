package vunly

import (
	"crypto/hmac"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"strconv"
	"strings"
	"time"
)

// WebhookTolerance is how far a signed timestamp may be from now (either way) before it counts as stale.
const WebhookTolerance = 300 * time.Second

// WebhookEvent is a verified webhook delivery.
type WebhookEvent struct {
	ID        string          `json:"id"`
	Type      string          `json:"type"`
	CreatedAt time.Time       `json:"createdAt"`
	Data      json.RawMessage `json:"data"`
}

// WebhookError is returned by VerifyWebhook. Reason is one of "missing_header", "malformed_header", "stale_timestamp",
// "no_matching_signature" or "invalid_payload". Answer a failed verification with a 400 and do not process the payload.
type WebhookError struct{ Reason string }

func (e *WebhookError) Error() string { return "vunly: webhook verification failed (" + e.Reason + ")" }

// ErrEmptyWebhookSecret is returned by VerifyWebhook when no secret is given or one of them is empty or blank: the delivery is
// not verified (with an empty key anyone can compute a valid signature). It is a configuration error, not a *WebhookError.
var ErrEmptyWebhookSecret = errors.New("vunly: the webhook secret is empty or missing (is its environment variable set?)")

type webhookOptions struct{ now func() time.Time }

// WebhookOption changes how VerifyWebhook behaves.
type WebhookOption func(*webhookOptions)

// WithWebhookClock replaces the clock, for tests.
func WithWebhookClock(now func() time.Time) WebhookOption {
	return func(o *webhookOptions) { o.now = now }
}

// VerifyWebhook checks a delivery. payload is the RAW request body (the exact bytes, not re-encoded JSON), header is the
// Vunly-Signature header and secrets are the endpoint's signing secrets: pass two while rotating, a match with any one
// is enough. The signature is HMAC-SHA256 over "<t>." + payload, compared in constant time.
func VerifyWebhook(payload []byte, header string, secrets []string, opts ...WebhookOption) (*WebhookEvent, error) {
	o := webhookOptions{now: time.Now}
	for _, f := range opts {
		f(&o)
	}
	// An empty secret signs anything anyone wants: a missing environment variable would make webhooks forgeable. A configuration
	// error, returned before any signature is looked at (a *WebhookError would be answered 400 and hide it).
	if len(secrets) == 0 {
		return nil, ErrEmptyWebhookSecret
	}
	for _, s := range secrets {
		if !hasPrintableASCII(s) {
			return nil, ErrEmptyWebhookSecret
		}
	}
	if strings.TrimSpace(header) == "" {
		return nil, &WebhookError{"missing_header"}
	}
	var ts string
	var sigs []string
	for _, part := range strings.Split(header, ",") {
		k, v, ok := strings.Cut(strings.TrimSpace(part), "=")
		if !ok {
			continue
		}
		switch k {
		case "t":
			ts = v
		case "v1":
			if isSignatureHex(v) { // a v1 that is not 64 hex characters is not a signature: the header is malformed
				sigs = append(sigs, v)
			}
		}
	}
	t, err := strconv.ParseInt(ts, 10, 64)
	if err != nil || len(sigs) == 0 {
		return nil, &WebhookError{"malformed_header"}
	}
	d := o.now().Sub(time.Unix(t, 0))
	if d < 0 {
		d = -d
	}
	if d > WebhookTolerance {
		return nil, &WebhookError{"stale_timestamp"}
	}
	prefix := []byte(ts + ".")
	matched := false
	for _, secret := range secrets {
		mac := hmac.New(sha256.New, []byte(secret))
		mac.Write(prefix) // the body is fed as it is, not copied behind the timestamp
		mac.Write(payload)
		want := mac.Sum(nil)
		for _, s := range sigs {
			got, err := hex.DecodeString(s)
			if err == nil && hmac.Equal(got, want) {
				matched = true
			}
		}
	}
	if !matched {
		return nil, &WebhookError{"no_matching_signature"}
	}
	var ev WebhookEvent
	if err := json.Unmarshal(payload, &ev); err != nil || ev.ID == "" || ev.Type == "" {
		return nil, &WebhookError{"invalid_payload"}
	}
	return &ev, nil
}

// hasPrintableASCII: a usable secret has at least one printable ASCII character that is not a space (HMAC-SHA256, 300 s tolerance).
func hasPrintableASCII(s string) bool {
	for i := 0; i < len(s); i++ {
		if s[i] >= 0x21 && s[i] <= 0x7e {
			return true
		}
	}
	return false
}

// isSignatureHex: a v1 value is 64 lowercase hexadecimal characters (HMAC-SHA256, as the API sends it).
func isSignatureHex(s string) bool {
	if len(s) != 64 {
		return false
	}
	for i := 0; i < len(s); i++ {
		c := s[i]
		if !(c >= '0' && c <= '9' || c >= 'a' && c <= 'f') {
			return false
		}
	}
	return true
}
