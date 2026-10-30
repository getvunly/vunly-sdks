package vunly_test

import (
	"context"
	"crypto/hmac"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/http/cookiejar"
	"net/http/httptest"
	"net/url"
	"os"
	"strconv"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	vunly "github.com/getvunly/vunly-sdks/go"
)

const testKey = "vk_test_synthetic_000000000000000000"

func noSleep(context.Context, time.Duration) error { return nil }

func newClient(t *testing.T, h http.HandlerFunc, opts ...vunly.Option) (*vunly.Client, *httptest.Server) {
	t.Helper()
	srv := httptest.NewServer(h)
	t.Cleanup(srv.Close)
	all := append([]vunly.Option{vunly.WithBaseURL(srv.URL), vunly.WithSleep(noSleep)}, opts...)
	c, err := vunly.New(testKey, all...)
	if err != nil {
		t.Fatal(err)
	}
	return c, srv
}

func writeJSON(w http.ResponseWriter, status int, body string) {
	w.Header().Set("Content-Type", "application/json")
	w.Header().Set("Request-Id", "req_test")
	w.WriteHeader(status)
	_, _ = io.WriteString(w, body)
}

const entry = `{"id":"led_1","customerId":"cus_1","kind":"debit","credits":-3,"createdAt":"2026-01-01T00:00:00Z"}`

func TestNewRequiresKeyAndRedactsIt(t *testing.T) {
	if _, err := vunly.New(""); err == nil {
		t.Fatal("empty key accepted")
	}
	c, err := vunly.New(testKey, vunly.WithBaseURL("http://127.0.0.1:1"))
	if err != nil {
		t.Fatal(err)
	}
	for _, f := range []string{"%v", "%+v", "%#v", "%s"} {
		if s := fmt.Sprintf(f, c); strings.Contains(s, testKey) {
			t.Fatalf("%s leaks the key: %s", f, s)
		}
	}
}

func TestRecordSendsKeyAndIdempotencyAndReportsOverage(t *testing.T) {
	c, _ := newClient(t, func(w http.ResponseWriter, r *http.Request) {
		if r.Header.Get("Authorization") != "Bearer "+testKey {
			t.Errorf("authorization = %q", r.Header.Get("Authorization"))
		}
		if r.Header.Get("Idempotency-Key") != "job-12345678" {
			t.Errorf("idempotency key = %q", r.Header.Get("Idempotency-Key"))
		}
		if !strings.HasPrefix(r.Header.Get("User-Agent"), "vunly-go/") {
			t.Errorf("user agent = %q", r.Header.Get("User-Agent"))
		}
		w.Header().Set("Vunly-Plan-Overage", "true")
		writeJSON(w, 200, `{"event":`+entry+`,"balance":97}`)
	})
	res, err := c.Usage.Record(context.Background(), vunly.RecordParams{ID: "job-12345678", Customer: "acme", Meter: "tokens", Quantity: 3000})
	if err != nil {
		t.Fatal(err)
	}
	if res.Remaining != 97 || !res.PlanOverage || res.Replayed || res.RequestID != "req_test" {
		t.Fatalf("unexpected result %+v", res)
	}
}

func TestRecordRejectsBadIDLocally(t *testing.T) {
	var hits atomic.Int32
	c, _ := newClient(t, func(w http.ResponseWriter, r *http.Request) { hits.Add(1) })
	_, err := c.Usage.Record(context.Background(), vunly.RecordParams{ID: "short", Customer: "a", Meter: "m", Quantity: 1})
	if !vunly.IsInvalidRequest(err) || hits.Load() != 0 {
		t.Fatalf("err=%v hits=%d", err, hits.Load())
	}
}

func TestLimitReachedCarriesBalanceAndIsNotRetried(t *testing.T) {
	var hits atomic.Int32
	c, _ := newClient(t, func(w http.ResponseWriter, r *http.Request) {
		hits.Add(1)
		writeJSON(w, 402, `{"error":{"code":"insufficient_credits","message":"not enough","details":{"balance":2,"required":5}}}`)
	})
	_, err := c.Usage.Record(context.Background(), vunly.RecordParams{ID: "job-12345678", Customer: "a", Meter: "m", Quantity: 1})
	var lr *vunly.LimitReachedError
	if !errors.As(err, &lr) {
		t.Fatalf("err = %v", err)
	}
	if b, _ := lr.Balance(); b != 2 {
		t.Fatalf("balance %d", b)
	}
	if r, _ := lr.Required(); r != 5 {
		t.Fatalf("required %d", r)
	}
	var api *vunly.APIError
	if !errors.As(err, &api) || api.RequestID != "req_test" || hits.Load() != 1 {
		t.Fatalf("api=%v hits=%d", api, hits.Load())
	}
}

func TestAuthAndInvalidRequestClasses(t *testing.T) {
	for status, check := range map[int]func(error) bool{401: vunly.IsAuth, 403: vunly.IsAuth, 404: vunly.IsInvalidRequest, 409: vunly.IsInvalidRequest, 400: vunly.IsInvalidRequest} {
		status, check := status, check
		c, _ := newClient(t, func(w http.ResponseWriter, r *http.Request) {
			writeJSON(w, status, `{"error":{"code":"x","message":"m"}}`)
		})
		if _, err := c.Customers.Get(context.Background(), "acme"); !check(err) {
			t.Fatalf("%d classified as %T", status, err)
		}
	}
}

func TestReadsRetryServerErrorsThenSucceed(t *testing.T) {
	var hits atomic.Int32
	c, _ := newClient(t, func(w http.ResponseWriter, r *http.Request) {
		if hits.Add(1) < 3 {
			writeJSON(w, 503, `{"error":{"code":"unavailable","message":"later"}}`)
			return
		}
		writeJSON(w, 200, `{"credits":5,"consumed":1,"granted":6}`)
	})
	b, err := c.Credits.Balance(context.Background(), "acme")
	if err != nil || b.Remaining != 5 || hits.Load() != 3 {
		t.Fatalf("b=%+v err=%v hits=%d", b, err, hits.Load())
	}
}

// A dropped reused connection must not be replayed by net/http behind the SDK's back: the Idempotency-Key header makes
// a POST look replayable to the transport.
func TestTransportDoesNotReplayADroppedRequest(t *testing.T) {
	var posts atomic.Int32
	c, _ := newClient(t, func(w http.ResponseWriter, r *http.Request) {
		if r.Method == http.MethodGet {
			writeJSON(w, 200, `{"credits":1,"consumed":0,"granted":1}`)
			return
		}
		posts.Add(1)
		conn, _, err := w.(http.Hijacker).Hijack()
		if err == nil {
			_ = conn.Close() // the request arrived, the answer never will
		}
	}, vunly.WithMaxRetries(0))
	if _, err := c.Credits.Balance(context.Background(), "acme"); err != nil { // warms a keep-alive connection
		t.Fatal(err)
	}
	_, err := c.Usage.Record(context.Background(), vunly.RecordParams{ID: "job-12345678", Customer: "a", Meter: "m", Quantity: 1})
	if !vunly.IsUnavailable(err) || posts.Load() != 1 {
		t.Fatalf("err=%v posts=%d (want Unavailable after exactly one attempt)", err, posts.Load())
	}
}

func TestRetriesAreBounded(t *testing.T) {
	var hits atomic.Int32
	c, _ := newClient(t, func(w http.ResponseWriter, r *http.Request) {
		hits.Add(1)
		writeJSON(w, 500, `{"error":{"code":"internal_error","message":"boom"}}`)
	}, vunly.WithMaxRetries(2))
	_, err := c.Credits.Balance(context.Background(), "acme")
	if !vunly.IsUnavailable(err) || hits.Load() != 3 {
		t.Fatalf("err=%v hits=%d", err, hits.Load())
	}
}

func TestClassXIsRetriedOnlyAfter429(t *testing.T) {
	var hits atomic.Int32
	c, _ := newClient(t, func(w http.ResponseWriter, r *http.Request) {
		hits.Add(1)
		writeJSON(w, 500, `{"error":{"code":"internal_error","message":"boom"}}`)
	})
	if _, err := c.CustomerTokens.Create(context.Background(), "acme", 0); !vunly.IsUnavailable(err) || hits.Load() != 1 {
		t.Fatalf("err=%v hits=%d", err, hits.Load())
	}

	hits.Store(0)
	c2, _ := newClient(t, func(w http.ResponseWriter, r *http.Request) {
		if hits.Add(1) == 1 {
			w.Header().Set("Retry-After", "1")
			writeJSON(w, 429, `{"error":{"code":"rate_limited","message":"slow"}}`)
			return
		}
		writeJSON(w, 200, `{"id":"ct_1","token":"vct_x","customerId":"cus_1","expiresAt":"2026-01-01T00:15:00Z"}`)
	})
	tok, err := c2.CustomerTokens.Create(context.Background(), "acme", 0)
	if err != nil || hits.Load() != 2 {
		t.Fatalf("err=%v hits=%d", err, hits.Load())
	}
	if s := fmt.Sprintf("%v %+v %#v", tok, tok, tok); strings.Contains(s, "vct_x") {
		t.Fatalf("token printed: %s", s)
	}
}

func TestPlanVolumeStopIsNeverRetried(t *testing.T) {
	var hits atomic.Int32
	c, _ := newClient(t, func(w http.ResponseWriter, r *http.Request) {
		hits.Add(1)
		w.Header().Set("Retry-After", "5")
		writeJSON(w, 429, `{"error":{"code":"rate_limited","message":"plan","details":{"reason":"plan_included_volume"}}}`)
	})
	_, err := c.Usage.Record(context.Background(), vunly.RecordParams{ID: "job-12345678", Customer: "a", Meter: "m", Quantity: 1})
	var u *vunly.UnavailableError
	if !errors.As(err, &u) || !u.PlanStop() || hits.Load() != 1 {
		t.Fatalf("err=%v hits=%d", err, hits.Load())
	}
}

func TestRetryAfterAboveCapIsSurfaced(t *testing.T) {
	var hits atomic.Int32
	c, _ := newClient(t, func(w http.ResponseWriter, r *http.Request) {
		hits.Add(1)
		w.Header().Set("Retry-After", "120")
		writeJSON(w, 429, `{"error":{"code":"rate_limited","message":"slow"}}`)
	})
	_, err := c.Credits.Balance(context.Background(), "acme")
	var api *vunly.APIError
	if !errors.As(err, &api) || api.RetryAfter != 120*time.Second || hits.Load() != 1 {
		t.Fatalf("err=%v hits=%d", err, hits.Load())
	}
}

func TestMalformedSuccessIsInvalidResponse(t *testing.T) {
	c, _ := newClient(t, func(w http.ResponseWriter, r *http.Request) { writeJSON(w, 200, `{"nope":1}`) })
	_, err := c.Credits.Balance(context.Background(), "acme")
	var api *vunly.APIError
	if !vunly.IsUnavailable(err) || !errors.As(err, &api) || api.Code != "invalid_response" {
		t.Fatalf("err=%v", err)
	}
}

func TestTimeoutIsUnavailable(t *testing.T) {
	c, _ := newClient(t, func(w http.ResponseWriter, r *http.Request) {
		select {
		case <-r.Context().Done():
		case <-time.After(2 * time.Second):
		}
	}, vunly.WithTimeout(50*time.Millisecond), vunly.WithMaxRetries(0))
	start := time.Now()
	_, err := c.Credits.Balance(context.Background(), "acme")
	if !vunly.IsUnavailable(err) || time.Since(start) > time.Second {
		t.Fatalf("err=%v after %s", err, time.Since(start))
	}
}

func TestContextCancelStopsTheCall(t *testing.T) {
	c, _ := newClient(t, func(w http.ResponseWriter, r *http.Request) { <-r.Context().Done() })
	ctx, cancel := context.WithCancel(context.Background())
	go func() { time.Sleep(30 * time.Millisecond); cancel() }()
	_, err := c.Credits.Balance(ctx, "acme")
	if err == nil || !errors.Is(err, context.Canceled) {
		t.Fatalf("err=%v", err)
	}
}

func TestRedirectsAreNotFollowed(t *testing.T) {
	var followed atomic.Int32
	target := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { followed.Add(1) }))
	defer target.Close()
	c, _ := newClient(t, func(w http.ResponseWriter, r *http.Request) { http.Redirect(w, r, target.URL, http.StatusFound) })
	if _, err := c.Credits.Balance(context.Background(), "acme"); err == nil || followed.Load() != 0 {
		t.Fatalf("err=%v followed=%d", err, followed.Load())
	}
}

func TestPagingIteratorIsLazy(t *testing.T) {
	var pages atomic.Int32
	c, _ := newClient(t, func(w http.ResponseWriter, r *http.Request) {
		n := pages.Add(1)
		next := "null"
		if n < 3 {
			next = `"c` + strconv.Itoa(int(n)) + `"`
		}
		writeJSON(w, 200, fmt.Sprintf(`{"data":[%s,%s],"nextCursor":%s}`, entry, entry, next))
	})
	count := 0
	for _, err := range c.Credits.AllLedger(context.Background(), "acme", 2) {
		if err != nil {
			t.Fatal(err)
		}
		count++
		if count == 3 {
			break
		}
	}
	if count != 3 || pages.Load() != 2 {
		t.Fatalf("count=%d pages=%d", count, pages.Load())
	}
}

func TestPagingStopsOnARepeatedCursorAndOnEndlessEmptyPages(t *testing.T) {
	for name, fresh := range map[string]bool{"repeated cursor": false, "empty pages with a fresh cursor": true} {
		t.Run(name, func(t *testing.T) {
			var pages atomic.Int32
			c, _ := newClient(t, func(w http.ResponseWriter, r *http.Request) {
				n := pages.Add(1)
				cursor := "same"
				if fresh {
					cursor = "c" + strconv.Itoa(int(n))
				}
				writeJSON(w, 200, fmt.Sprintf(`{"data":[],"nextCursor":%q}`, cursor))
			})
			var got error
			for _, err := range c.Credits.AllLedger(context.Background(), "acme", 2) {
				got = err
			}
			var u *vunly.UnavailableError
			// Exactly 3 consecutive empty pages end the walk (a repeated cursor ends it on the second page): a 4th page
			// being fetched would be a weaker bound.
			want := int32(3)
			if !fresh {
				want = 2
			}
			if !errors.As(got, &u) || u.Code != "invalid_response" || pages.Load() != want {
				t.Fatalf("err=%v pages=%d, want exactly %d", got, pages.Load(), want)
			}
		})
	}
}

func TestAnIdThatWouldChangeTheRouteIsRefusedBeforeAnyRequest(t *testing.T) {
	var hits atomic.Int32
	c, _ := newClient(t, func(w http.ResponseWriter, r *http.Request) {
		hits.Add(1)
		writeJSON(w, 200, `{}`)
	})
	for _, bad := range []string{"", ".", ".."} {
		if _, err := c.Customers.Get(context.Background(), bad); err == nil {
			t.Errorf("Customers.Get(%q) was accepted", bad)
		}
		if _, err := c.Credits.Balance(context.Background(), bad); err == nil {
			t.Errorf("Credits.Balance(%q) was accepted", bad)
		}
	}
	if hits.Load() != 0 {
		t.Fatalf("%d requests were sent", hits.Load())
	}
}

func TestPathSegmentsAreEscaped(t *testing.T) {
	c, _ := newClient(t, func(w http.ResponseWriter, r *http.Request) {
		if r.URL.EscapedPath() != "/v1/customers/a%2Fb%3Fc/balance" {
			t.Errorf("path = %s", r.URL.EscapedPath())
		}
		writeJSON(w, 200, `{"credits":0,"consumed":0,"granted":0}`)
	})
	if _, err := c.Credits.Balance(context.Background(), "a/b?c"); err != nil {
		t.Fatal(err)
	}
}

func TestBaseURLWithV1Suffix(t *testing.T) {
	if got := vunly.NormalizeBaseURL("https://api.example.test/v1/"); got != "https://api.example.test" {
		t.Fatalf("got %q", got)
	}
}

func sign(secret string, t int64, payload string) string {
	m := hmac.New(sha256.New, []byte(secret))
	fmt.Fprintf(m, "%d.%s", t, payload)
	return fmt.Sprintf("t=%d,v1=%s", t, hex.EncodeToString(m.Sum(nil)))
}

func TestVerifyWebhookRefusesAnEmptySecretInsteadOfAcceptingAForgery(t *testing.T) {
	payload := []byte(`{"id":"evt_1","type":"webhook.test","createdAt":"2026-10-28T00:00:00Z","data":{}}`)
	now := time.Unix(1_800_000_000, 0)
	// what an attacker computes when the secret is the empty string
	mac := hmac.New(sha256.New, []byte(""))
	mac.Write([]byte("1800000000." + string(payload)))
	forged := "t=1800000000,v1=" + hex.EncodeToString(mac.Sum(nil))
	for _, secrets := range [][]string{nil, {}, {""}, {"  "}, {"whsec_synthetic_real", ""}, {" ", "whsec_synthetic_real"}} {
		ev, err := vunly.VerifyWebhook(payload, forged, secrets, vunly.WithWebhookClock(func() time.Time { return now }))
		if ev != nil || !errors.Is(err, vunly.ErrEmptyWebhookSecret) {
			t.Fatalf("secrets %q: event %v, err %v", secrets, ev, err)
		}
		var we *vunly.WebhookError
		if errors.As(err, &we) {
			t.Fatalf("an empty secret is a configuration error, not a verification failure: %v", err)
		}
	}
}

func TestVerifyWebhook(t *testing.T) {
	now := time.Unix(1_800_000_000, 0)
	clock := vunly.WithWebhookClock(func() time.Time { return now })
	payload := `{"id":"evt_1","type":"credits.low","createdAt":"2026-01-01T00:00:00Z","data":{"a":1}}`
	reason := func(err error) string {
		var we *vunly.WebhookError
		if errors.As(err, &we) {
			return we.Reason
		}
		return fmt.Sprint(err)
	}

	ev, err := vunly.VerifyWebhook([]byte(payload), sign("old", now.Unix(), payload), []string{"new", "old"}, clock)
	if err != nil || ev.ID != "evt_1" || ev.Type != "credits.low" {
		t.Fatalf("ev=%+v err=%v", ev, err)
	}
	cases := []struct{ name, header, want string }{
		{"missing", "", "missing_header"},
		{"malformed", "garbage", "malformed_header"},
		{"stale", sign("s", now.Unix()-301, payload), "stale_timestamp"},
		{"future", sign("s", now.Unix()+301, payload), "stale_timestamp"},
		{"wrong secret", sign("other", now.Unix(), payload), "no_matching_signature"},
	}
	for _, c := range cases {
		if _, err := vunly.VerifyWebhook([]byte(payload), c.header, []string{"s"}, clock); reason(err) != c.want {
			t.Errorf("%s: got %s want %s", c.name, reason(err), c.want)
		}
	}
	if _, err := vunly.VerifyWebhook([]byte(payload+" "), sign("s", now.Unix(), payload), []string{"s"}, clock); reason(err) != "no_matching_signature" {
		t.Errorf("tampered body accepted: %v", err)
	}
	bad := `{"nope":true}`
	if _, err := vunly.VerifyWebhook([]byte(bad), sign("s", now.Unix(), bad), []string{"s"}, clock); reason(err) != "invalid_payload" {
		t.Errorf("invalid payload: %v", err)
	}
}

// ---- regression tests for hostile or malformed input: each case failed before it was fixed ----

func TestF1EmptyIDIsRefusedLocallyForEveryKeyedCall(t *testing.T) {
	var hits atomic.Int32
	c, _ := newClient(t, func(w http.ResponseWriter, r *http.Request) { hits.Add(1); writeJSON(w, 200, `{}`) })
	calls := map[string]func() error{
		"Usage.Record": func() error {
			_, err := c.Usage.Record(context.Background(), vunly.RecordParams{Customer: "a", Meter: "m", Quantity: 1})
			return err
		},
		"Credits.Grant": func() error {
			_, err := c.Credits.Grant(context.Background(), vunly.GrantParams{Customer: "a", Credits: 1})
			return err
		},
	}
	for name, call := range calls {
		err := call()
		var inv *vunly.InvalidRequestError
		if !errors.As(err, &inv) {
			t.Fatalf("%s: err = %v, want *InvalidRequestError", name, err)
		}
		if inv.Status != 400 || inv.Code != "validation_error" {
			t.Fatalf("%s: %d %s", name, inv.Status, inv.Code)
		}
		if _, ok := inv.Details["fields"].(map[string]any)["id"]; !ok {
			t.Fatalf("%s: details.fields.id missing: %v", name, inv.Details)
		}
	}
	if hits.Load() != 0 {
		t.Fatalf("%d requests reached the server", hits.Load())
	}
}

func TestF2ACopiedClientDoesNotPrintTheKey(t *testing.T) {
	c, err := vunly.New(testKey, vunly.WithBaseURL("http://127.0.0.1:1"))
	if err != nil {
		t.Fatal(err)
	}
	copied := *c //nolint:govet // the point of the test
	for _, v := range []any{c, copied, &copied} {
		for _, f := range []string{"%v", "%+v", "%#v", "%s", "%q", "%x"} {
			if s := fmt.Sprintf(f, v); strings.Contains(s, testKey) || strings.Contains(s, "vk_test_synthetic") {
				t.Fatalf("%s of %T leaks the key: %s", f, v, s)
			}
		}
	}
}

func TestF3HostileRetryAfterNeverOverflowsOrWaits(t *testing.T) {
	for _, v := range []string{"99999999999", "1e12", "1e300", "Inf", "+Inf", "9223372036"} {
		var hits atomic.Int32
		c, _ := newClient(t, func(w http.ResponseWriter, r *http.Request) {
			hits.Add(1)
			w.Header().Set("Retry-After", v)
			writeJSON(w, 429, `{"error":{"code":"rate_limited","message":"slow"}}`)
		})
		_, err := c.Credits.Balance(context.Background(), "acme")
		var api *vunly.APIError
		if !errors.As(err, &api) {
			t.Fatalf("Retry-After %q: err = %v", v, err)
		}
		if api.RetryAfter <= 30*time.Second {
			t.Fatalf("Retry-After %q became %s: it must stay above the 30 s the SDK will wait (an overflow wraps negative)", v, api.RetryAfter)
		}
		if hits.Load() != 1 {
			t.Fatalf("Retry-After %q: %d requests; a wait above 30 s must be surfaced, not slept on", v, hits.Load())
		}
	}
	for _, v := range []string{"NaN", "-5", "abc", ""} {
		c, _ := newClient(t, func(w http.ResponseWriter, r *http.Request) {
			w.Header().Set("Retry-After", v)
			writeJSON(w, 429, `{"error":{"code":"rate_limited","message":"slow"}}`)
		}, vunly.WithMaxRetries(0))
		_, err := c.Credits.Balance(context.Background(), "acme")
		var api *vunly.APIError
		if !errors.As(err, &api) || api.RetryAfter != 0 {
			t.Fatalf("Retry-After %q should mean none: err = %v", v, err)
		}
	}
}

// The shared vectors (conformance/retry-after-vectors.json, copied here because the module is built alone in its container): a number
// above the 30 s cap, or too large for a float64 ("1e999", 400 digits), is a very long wait and is surfaced at once, never
// retried early; a number up to 30 is the wait; anything else is no header.
func TestRetryAfterSharedVectors(t *testing.T) {
	raw, err := os.ReadFile("testdata/retry-after-vectors.json")
	if err != nil {
		t.Fatal(err)
	}
	var file struct {
		Vectors []struct {
			Header  string  `json:"header"`
			Kind    string  `json:"kind"`
			Seconds float64 `json:"seconds"`
		} `json:"vectors"`
	}
	if err := json.Unmarshal(raw, &file); err != nil {
		t.Fatal(err)
	}
	if len(file.Vectors) < 30 {
		t.Fatalf("only %d vectors", len(file.Vectors))
	}
	for _, v := range file.Vectors {
		var hits atomic.Int32
		opts := []vunly.Option{}
		if v.Kind != "surface" {
			opts = append(opts, vunly.WithMaxRetries(0)) // do not sleep through backoff for the cases that are not about retrying
		}
		c, _ := newClient(t, func(w http.ResponseWriter, r *http.Request) {
			hits.Add(1)
			w.Header()["Retry-After"] = []string{v.Header}
			writeJSON(w, 429, `{"error":{"code":"rate_limited","message":"slow"}}`)
		}, opts...)
		_, err := c.Credits.Balance(context.Background(), "acme")
		var api *vunly.APIError
		if !errors.As(err, &api) {
			t.Fatalf("Retry-After %.30q: err = %v", v.Header, err)
		}
		switch v.Kind {
		case "seconds":
			if want := time.Duration(v.Seconds * float64(time.Second)); api.RetryAfter != want {
				t.Fatalf("Retry-After %q: %s, want %s", v.Header, api.RetryAfter, want)
			}
		case "surface":
			if want := time.Duration(v.Seconds * float64(time.Second)); api.RetryAfter != want {
				t.Fatalf("Retry-After %.30q: reported %s, want %s", v.Header, api.RetryAfter, want)
			}
			if hits.Load() != 1 {
				t.Fatalf("Retry-After %.30q: %d requests; a very long wait must be surfaced at once, never retried early", v.Header, hits.Load())
			}
		case "ignored":
			if api.RetryAfter != 0 {
				t.Fatalf("Retry-After %.30q should mean none, got %s", v.Header, api.RetryAfter)
			}
		default:
			t.Fatalf("unknown kind %q", v.Kind)
		}
	}
}

func TestF4ACallerDeadlineIsUnavailableAndCancelIsNot(t *testing.T) {
	c, _ := newClient(t, func(w http.ResponseWriter, r *http.Request) { <-r.Context().Done() }, vunly.WithMaxRetries(0))

	dctx, cancel := context.WithTimeout(context.Background(), 40*time.Millisecond)
	defer cancel()
	err := func() error { _, err := c.Credits.Balance(dctx, "acme"); return err }()
	if !vunly.IsUnavailable(err) {
		t.Fatalf("an expired caller deadline should be *UnavailableError, got %T %v", err, err)
	}
	if !errors.Is(err, context.DeadlineExceeded) {
		t.Fatalf("the deadline must stay visible to errors.Is: %v", err)
	}

	cctx, stop := context.WithCancel(context.Background())
	go func() { time.Sleep(30 * time.Millisecond); stop() }()
	err = func() error { _, err := c.Credits.Balance(cctx, "acme"); return err }()
	if vunly.IsUnavailable(err) || !errors.Is(err, context.Canceled) {
		t.Fatalf("a cancel is the caller's own decision and stays the context error, got %T %v", err, err)
	}
}

func TestARedirectIsUnavailableNotAValidationError(t *testing.T) {
	for _, status := range []int{301, 302, 307, 308} {
		var hits atomic.Int32
		c, _ := newClient(t, func(w http.ResponseWriter, r *http.Request) {
			hits.Add(1)
			w.Header().Set("Location", "https://elsewhere.example/v1/customers")
			w.WriteHeader(status)
		}) // default retries: a 3xx must still be sent exactly once
		_, err := c.Customers.Get(context.Background(), "acme")
		var api *vunly.APIError
		if !errors.As(err, &api) || !vunly.IsUnavailable(err) || vunly.IsInvalidRequest(err) || hits.Load() != 1 {
			t.Fatalf("%d: err = %v", status, err)
		}
		if api.Code != "unexpected_redirect" || api.Status != status {
			t.Fatalf("%d: code %q status %d", status, api.Code, api.Status)
		}
		if !strings.Contains(api.Message, "base URL") || strings.Contains(api.Message, "elsewhere.example") {
			t.Fatalf("%d: the message should point at the base URL and not echo the target: %s", status, api.Message)
		}
	}
}

// Every attempt of a keyed mutation carries the SAME Idempotency-Key, or a retry could apply the work twice.
func TestRetriesSendTheSameIdempotencyKeyEveryTime(t *testing.T) {
	var mu sync.Mutex
	var keys []string
	c, _ := newClient(t, func(w http.ResponseWriter, r *http.Request) {
		mu.Lock()
		keys = append(keys, r.Header.Get("Idempotency-Key"))
		n := len(keys)
		mu.Unlock()
		if n < 3 {
			writeJSON(w, 503, `{"error":{"code":"unavailable","message":"later"}}`)
			return
		}
		writeJSON(w, 200, `{"event":`+entry+`,"balance":97}`)
	})
	if _, err := c.Usage.Record(context.Background(), vunly.RecordParams{ID: "job-12345678", Customer: "acme", Meter: "tokens", Quantity: 3}); err != nil {
		t.Fatal(err)
	}
	if len(keys) != 3 {
		t.Fatalf("attempts = %d, want 3", len(keys))
	}
	for i, k := range keys {
		if k != "job-12345678" {
			t.Errorf("attempt %d sent Idempotency-Key %q", i+1, k)
		}
	}
}

// A server's Retry-After is what the SDK waits, not its own backoff.
func TestRetryAfterIsWaitedBeforeTheRetry(t *testing.T) {
	var hits atomic.Int32
	var waits []time.Duration
	sleep := func(_ context.Context, d time.Duration) error { waits = append(waits, d); return nil }
	c, _ := newClient(t, func(w http.ResponseWriter, r *http.Request) {
		if hits.Add(1) == 1 {
			w.Header().Set("Retry-After", "7")
			writeJSON(w, 429, `{"error":{"code":"rate_limited","message":"slow"}}`)
			return
		}
		writeJSON(w, 200, `{"credits":5,"consumed":1,"granted":6}`)
	}, vunly.WithSleep(sleep))
	if _, err := c.Credits.Balance(context.Background(), "acme"); err != nil {
		t.Fatal(err)
	}
	if len(waits) != 1 || waits[0] != 7*time.Second {
		t.Fatalf("waits = %v, want exactly [7s]", waits)
	}
}

// What the server says is text of unknown origin: cleaned, bounded, and the client's own key is never repeated.
func TestServerTextIsCleanedBoundedAndTheKeyRedacted(t *testing.T) {
	// A right-to-left override and a zero-width space; a zero-width character must not hide the key.
	bidi := string(rune(0x202e))
	zw := string(rune(0x200b))
	splitKey := testKey[:10] + zw + testKey[10:]
	raw, merr := json.Marshal(map[string]any{"error": map[string]any{
		"code":      "bad" + bidi + "_code" + testKey + strings.Repeat("c", 100),
		"message":   "hello\u0007" + bidi + " world " + testKey + " and " + splitKey + " " + strings.Repeat("m", 600),
		"requestId": "req" + zw + "_" + testKey + strings.Repeat("r", 200),
		"details": map[string]any{
			"hint":          "x\x01" + testKey + bidi,
			"nested":        []any{map[string]any{"k": "deep " + testKey}},
			"key" + testKey: 1,
		},
	}})
	if merr != nil {
		t.Fatal(merr)
	}
	body := string(raw)
	c, _ := newClient(t, func(w http.ResponseWriter, r *http.Request) { writeJSON(w, 400, body) }, vunly.WithMaxRetries(0))
	_, err := c.Customers.Get(context.Background(), "acme")
	var api *vunly.APIError
	if !errors.As(err, &api) {
		t.Fatalf("err = %v", err)
	}
	if len([]rune(api.Message)) > 500 || len([]rune(api.Code)) > 64 || len([]rune(api.RequestID)) > 128 {
		t.Fatalf("not bounded: message %d, code %d, request id %d", len([]rune(api.Message)), len([]rune(api.Code)), len([]rune(api.RequestID)))
	}
	all := api.Message + "|" + api.Code + "|" + api.RequestID + "|" + fmt.Sprint(api.Details) + "|" + err.Error()
	for _, bad := range []string{testKey, bidi, zw, "\u0007", "\x01", testKey[:10]} {
		if strings.Contains(all, bad) {
			t.Fatalf("%q survives in %q", bad, all)
		}
	}
	if !strings.HasPrefix(api.Message, "hello world [redacted] and [redacted]") {
		t.Fatalf("message = %q", api.Message)
	}
	if !strings.HasPrefix(api.Code, "bad_code[redacted]") {
		t.Fatalf("code = %q", api.Code)
	}
}

// The Request-Id header of a success is shown to people too.
func TestSuccessRequestIDIsCleanedBoundedAndRedacted(t *testing.T) {
	c, _ := newClient(t, func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		w.Header().Set("Request-Id", "r"+string(rune(0x202e))+"q-"+testKey+"-"+strings.Repeat("z", 300))
		_, _ = io.WriteString(w, `{"event":`+entry+`,"balance":1}`)
	})
	res, err := c.Usage.Record(context.Background(), vunly.RecordParams{ID: "job-12345678", Customer: "acme", Meter: "tokens", Quantity: 1})
	if err != nil {
		t.Fatal(err)
	}
	if strings.Contains(res.RequestID, testKey) || strings.ContainsRune(res.RequestID, 0x202e) || len([]rune(res.RequestID)) > 128 || !strings.HasPrefix(res.RequestID, "rq-[redacted]-") {
		t.Fatalf("request id = %q", res.RequestID)
	}
}

// Whatever the body says, a 3xx is unexpected_redirect: not the body's code, not retried.
func TestARedirectWithAnErrorBodyIsStillUnexpectedRedirect(t *testing.T) {
	var hits atomic.Int32
	c, _ := newClient(t, func(w http.ResponseWriter, r *http.Request) {
		hits.Add(1)
		w.Header().Set("Location", "https://elsewhere.example/")
		writeJSON(w, 302, `{"error":{"code":"insufficient_credits","message":"pay me","requestId":"req_x","details":{"balance":1}}}`)
	})
	_, err := c.Credits.Balance(context.Background(), "acme")
	var api *vunly.APIError
	if !errors.As(err, &api) || !vunly.IsUnavailable(err) || api.Code != "unexpected_redirect" || api.Status != 302 || strings.Contains(api.Message, "pay me") || api.Details != nil || hits.Load() != 1 {
		t.Fatalf("err=%v hits=%d", err, hits.Load())
	}
}

// The server's details are bounded: 50 entries per object or array, and 5 levels (the top object is level 0, so the
// object under "deep" is level 1 and four nested objects survive; the fifth value is cut to null).
func TestDetailsAreBoundedInEntriesAndDepth(t *testing.T) {
	list := make([]any, 120)
	many := map[string]any{}
	for i := range list {
		list[i] = float64(i)
		many[fmt.Sprintf("k%03d", i)] = float64(i)
	}
	var deep any = map[string]any{"leaf": "x"}
	for i := 0; i < 9; i++ {
		deep = map[string]any{"n": deep}
	}
	raw, merr := json.Marshal(map[string]any{"error": map[string]any{
		"code": "validation_error", "message": "m",
		"details": map[string]any{"list": list, "many": many, "deep": deep},
	}})
	if merr != nil {
		t.Fatal(merr)
	}
	c, _ := newClient(t, func(w http.ResponseWriter, r *http.Request) { writeJSON(w, 400, string(raw)) }, vunly.WithMaxRetries(0))
	_, err := c.Customers.Get(context.Background(), "acme")
	var api *vunly.APIError
	if !errors.As(err, &api) {
		t.Fatalf("err = %v", err)
	}
	got, ok := api.Details["list"].([]any)
	if !ok || len(got) != 50 || got[49] != float64(49) {
		t.Fatalf("list = %d entries, want exactly the first 50", len(got))
	}
	gotMany, ok := api.Details["many"].(map[string]any)
	if !ok || len(gotMany) != 50 {
		t.Fatalf("many = %d entries, want 50", len(gotMany))
	}
	if _, ok := gotMany["k049"]; !ok {
		t.Fatalf("many lost k049 (the first 50 keys in order stay)")
	}
	if _, ok := gotMany["k050"]; ok {
		t.Fatalf("many kept k050")
	}
	levels := 0
	node := api.Details["deep"]
	for {
		m, ok := node.(map[string]any)
		if !ok {
			break
		}
		levels++
		node = m["n"]
	}
	if levels != 4 || node != nil {
		t.Fatalf("deep: %d object levels survive (want 4), then %v (want nil)", levels, node)
	}
}

// Whatever redirect policy a caller's own client has, the SDK never follows a 3xx (a followed redirect would re-send the
// key to the same host or to a sibling address). The policies below try to look like they stop but follow in practice;
// each must end in unexpected_redirect with exactly one request, and the caller's client must stay as it was.
func TestWithHTTPClientNeverFollowsRedirectsWhateverThePolicy(t *testing.T) {
	var other atomic.Int32
	sibling := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { other.Add(1) }))
	t.Cleanup(sibling.Close)

	policies := map[string]func() func(*http.Request, []*http.Request) error{
		"nil policy": func() func(*http.Request, []*http.Request) error { return nil },
		"explicit follow": func() func(*http.Request, []*http.Request) error {
			return func(*http.Request, []*http.Request) error { return nil }
		},
		"stops only on its first call": func() func(*http.Request, []*http.Request) error {
			var calls int
			f := func(*http.Request, []*http.Request) error {
				calls++
				if calls == 1 {
					return http.ErrUseLastResponse
				}
				return nil
			}
			_ = f(nil, nil) // the one question a probe at construction would have asked
			return f
		},
		"looks at Authorization in via[0]": func() func(*http.Request, []*http.Request) error {
			return func(_ *http.Request, via []*http.Request) error {
				// A probe request carries no Authorization; the SDK's real ones do.
				if len(via) > 0 && via[0].Header.Get("Authorization") == "" {
					return http.ErrUseLastResponse
				}
				return nil
			}
		},
		"stops only for the base path": func() func(*http.Request, []*http.Request) error {
			return func(req *http.Request, _ []*http.Request) error {
				if req.URL.Path == "" || req.URL.Path == "/" {
					return http.ErrUseLastResponse
				}
				return nil
			}
		},
	}
	targets := map[string]func() (string, *atomic.Int32){
		"same origin":     func() (string, *atomic.Int32) { return "/again", nil },
		"another address": func() (string, *atomic.Int32) { return sibling.URL + "/x", &other },
	}
	for pname, mk := range policies {
		for tname, tgt := range targets {
			t.Run(pname+"/"+tname, func(t *testing.T) {
				other.Store(0)
				loc, watched := tgt()
				var hits atomic.Int32
				var location atomic.Value
				var seen sync.Map
				location.Store(loc)
				srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
					hits.Add(1)
					seen.Store(r.Method+" "+r.URL.Path, true)
					if r.URL.Path == "/final" {
						w.WriteHeader(200) // the end of the chain: no loop if the caller's client follows
						return
					}
					w.Header().Set("Location", location.Load().(string))
					w.WriteHeader(302)
				}))
				t.Cleanup(srv.Close)
				jar, err := cookiejar.New(nil)
				if err != nil {
					t.Fatal(err)
				}
				transport := &http.Transport{}
				policy := mk()
				orig := &http.Client{Transport: transport, Jar: jar, Timeout: 7 * time.Second, CheckRedirect: policy}
				c, err := vunly.New(testKey, vunly.WithBaseURL(srv.URL), vunly.WithHTTPClient(orig), vunly.WithSleep(noSleep))
				if err != nil {
					t.Fatalf("New: %v", err)
				}
				_, err = c.Credits.Balance(context.Background(), "acme")
				var api *vunly.APIError
				if !errors.As(err, &api) || api.Code != "unexpected_redirect" || !vunly.IsUnavailable(err) {
					t.Fatalf("err=%v, want unexpected_redirect of class Unavailable", err)
				}
				if hits.Load() != 1 {
					var paths []string
					seen.Range(func(k, _ any) bool { paths = append(paths, k.(string)); return true })
					t.Fatalf("%d requests reached the server (%v), want exactly 1", hits.Load(), paths)
				}
				if watched != nil && watched.Load() != 0 {
					t.Fatalf("the other address received %d requests", watched.Load())
				}
				// The caller's client is intact.
				if orig.Transport != http.RoundTripper(transport) || orig.Jar != jar || orig.Timeout != 7*time.Second {
					t.Fatalf("the caller's Transport, Jar or Timeout changed")
				}
				if (orig.CheckRedirect == nil) != (policy == nil) {
					t.Fatalf("the caller's CheckRedirect was replaced")
				}
				// Used directly, the original still follows redirects.
				location.Store("/final")
				hits.Store(0)
				resp, err := orig.Get(srv.URL + "/start")
				if err == nil {
					resp.Body.Close()
				}
				// The policies that follow see exactly two requests (the first and the followed one); the one that
				// stops on a request without Authorization sees one.
				want := int32(2)
				if pname == "looks at Authorization in via[0]" {
					want = 1
				}
				if hits.Load() != want {
					t.Fatalf("the caller's client, used directly, made %d requests, want exactly %d", hits.Load(), want)
				}
			})
		}
	}
}

// The copy keeps the caller's Transport (proxy settings, TLS), cookie Jar and Timeout.
func TestWithHTTPClientKeepsTransportJarAndTimeout(t *testing.T) {
	var viaTransport atomic.Int32
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		_, _ = io.WriteString(w, `{"credits":0,"consumed":0,"granted":0}`)
	}))
	t.Cleanup(srv.Close)
	jar, err := cookiejar.New(nil)
	if err != nil {
		t.Fatal(err)
	}
	u, _ := url.Parse(srv.URL)
	jar.SetCookies(u, []*http.Cookie{{Name: "probe", Value: "1", Path: "/"}})
	var gotCookie atomic.Value
	gotCookie.Store("")
	counting := roundTripFunc(func(r *http.Request) (*http.Response, error) {
		viaTransport.Add(1)
		gotCookie.Store(r.Header.Get("Cookie"))
		return http.DefaultTransport.RoundTrip(r)
	})
	orig := &http.Client{Transport: counting, Jar: jar, Timeout: 5 * time.Second}
	c, err := vunly.New(testKey, vunly.WithBaseURL(srv.URL), vunly.WithHTTPClient(orig), vunly.WithSleep(noSleep))
	if err != nil {
		t.Fatal(err)
	}
	if _, err := c.Credits.Balance(context.Background(), "acme"); err != nil {
		t.Fatal(err)
	}
	if viaTransport.Load() == 0 {
		t.Fatalf("the caller's Transport was not used")
	}
	if got, _ := gotCookie.Load().(string); !strings.Contains(got, "probe=1") {
		t.Fatalf("the caller's Jar was not used (Cookie header %q)", got)
	}
	if orig.CheckRedirect != nil {
		t.Fatalf("the caller's client was modified")
	}
}

type roundTripFunc func(*http.Request) (*http.Response, error)

func (f roundTripFunc) RoundTrip(r *http.Request) (*http.Response, error) { return f(r) }
