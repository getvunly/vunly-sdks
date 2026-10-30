// Package vunly is the Go client for the Vunly API: customers, credits, usage, entitlement checks, plans and webhooks.
// Sandbox only for now. No third-party dependencies.
//
// The loop every integration is built from:
//
//	c, err := vunly.New(os.Getenv("VUNLY_API_KEY"), vunly.WithBaseURL(os.Getenv("VUNLY_API_URL")))
//	ent, err := c.Entitlements.Check(ctx, vunly.CheckParams{Customer: "acme", Feature: "generate"})
//	if err != nil || !ent.Allowed { /* do not run the work; a failed check is a "no" */ }
//	// ... run the work ...
//	res, err := c.Usage.Record(ctx, vunly.RecordParams{ID: requestID, Customer: "acme", Meter: "tokens", Quantity: 1050})
//
// The check is advisory and fails closed; the record is final. The ID names the unit of work (your request or job id),
// is sent as the Idempotency-Key, and is never generated for you: retry with the SAME one.
package vunly

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"math"
	"math/rand/v2"
	"net/http"
	"net/url"
	"regexp"
	"runtime"
	"strconv"
	"strings"
	"time"
)

// secret holds the API key and never prints itself, however it is formatted.
type secret string

func (secret) String() string               { return "<hidden>" }
func (secret) GoString() string             { return "<hidden>" }
func (secret) MarshalJSON() ([]byte, error) { return []byte(`"<hidden>"`), nil }
func (secret) MarshalText() ([]byte, error) { return []byte("<hidden>"), nil }
func (secret) Format(f fmt.State, _ rune)   { _, _ = io.WriteString(f, "<hidden>") }

// Client talks to one Vunly API. It is safe for concurrent use. Create it once and share it.
type Client struct {
	apiKey     secret
	baseURL    string
	http       *http.Client
	timeout    time.Duration
	maxRetries int
	sleep      func(context.Context, time.Duration) error
	userAgent  string

	// Customers creates and reads the customers you bill.
	Customers *CustomersService
	// Credits grants credits and reads balances and the ledger.
	Credits *CreditsService
	// Usage records usage events.
	Usage *UsageService
	// Entitlements asks whether a customer may do something now.
	Entitlements *EntitlementsService
	// Plans manages meters, plans and subscriptions.
	Plans *PlansService
	// CustomerTokens mints short-lived read tokens for one customer, for a browser.
	CustomerTokens *CustomerTokensService
}

// Option configures New.
type Option func(*Client)

// WithBaseURL sets the API address, for example "https://host" or "http://127.0.0.1:8787". A trailing slash or "/v1"
// is accepted and ignored. Required: the SDK has no default address.
func WithBaseURL(u string) Option { return func(c *Client) { c.baseURL = u } }

// WithHTTPClient replaces the HTTP client (proxies, custom transports, tests). The SDK sets its own per-attempt timeout
// with the request context. The SDK uses a copy of your client with its Transport, Jar and Timeout, and forces the
// redirect policy: a 3xx answer is never followed (it would send your key to wherever it points) and
// ends in unexpected_redirect, whatever your CheckRedirect says. Your own client is not modified.
func WithHTTPClient(h *http.Client) Option { return func(c *Client) { c.http = h } }

// WithTimeout sets the per-attempt timeout (default 10 seconds). Your context's deadline still applies on top.
func WithTimeout(d time.Duration) Option { return func(c *Client) { c.timeout = d } }

// WithMaxRetries sets how many times a retryable failure is repeated (default 2; 0 turns retries off).
func WithMaxRetries(n int) Option { return func(c *Client) { c.maxRetries = n } }

// WithSleep replaces the wait between retries (tests).
func WithSleep(f func(context.Context, time.Duration) error) Option {
	return func(c *Client) { c.sleep = f }
}

// New returns a client. The key and the base URL are required; neither is read from the environment or from disk.
func New(apiKey string, opts ...Option) (*Client, error) {
	c := &Client{
		apiKey:     secret(apiKey),
		timeout:    10 * time.Second,
		maxRetries: 2,
		sleep:      sleepCtx,
		userAgent:  fmt.Sprintf("vunly-go/%s (%s %s/%s)", Version, runtime.Version(), runtime.GOOS, runtime.GOARCH),
	}
	for _, o := range opts {
		o(c)
	}
	if strings.TrimSpace(apiKey) == "" {
		return nil, errors.New("vunly: the API key is empty (read it from your environment)")
	}
	base, err := normalizeBaseURL(c.baseURL)
	if err != nil {
		return nil, err
	}
	c.baseURL = base
	if c.maxRetries < 0 {
		return nil, errors.New("vunly: WithMaxRetries must not be negative")
	}
	if c.timeout <= 0 {
		return nil, errors.New("vunly: WithTimeout must be positive")
	}
	if c.http == nil {
		c.http = &http.Client{}
	}
	// Whatever the caller's policy, a 3xx is handed back and never followed: work on a shallow copy so the caller's own
	// client is not modified, and keep its Transport, Jar and Timeout.
	hc := *c.http
	hc.CheckRedirect = func(*http.Request, []*http.Request) error { return http.ErrUseLastResponse }
	c.http = &hc
	c.Customers = &CustomersService{c}
	c.Credits = &CreditsService{c}
	c.Usage = &UsageService{c}
	c.Entitlements = &EntitlementsService{c}
	c.Plans = &PlansService{c}
	c.CustomerTokens = &CustomerTokensService{c}
	return c, nil
}

// String never contains the key. It and Format are on the VALUE receiver on purpose: a Client that was copied
// (`cp := *c`) must not print the key either, and with a pointer receiver fmt would fall back to walking its unexported
// fields, where it cannot call secret's own methods.
func (c Client) String() string {
	return fmt.Sprintf("vunly.Client{baseURL: %q, apiKey: <hidden>}", c.baseURL)
}

// Format makes every verb (%v, %+v, %#v, %s, %q) print String, for a Client and for a *Client.
func (c Client) Format(f fmt.State, _ rune) { _, _ = io.WriteString(f, c.String()) }

var versionSuffix = regexp.MustCompile(`(?i)/v1$`)

// NormalizeBaseURL returns the address without a trailing slash and without a trailing "/v1": the SDK adds "/v1/..." itself.
func NormalizeBaseURL(u string) string {
	out := strings.TrimRight(strings.TrimSpace(u), "/")
	out = versionSuffix.ReplaceAllString(out, "")
	return strings.TrimRight(out, "/")
}

func normalizeBaseURL(u string) (string, error) {
	if strings.TrimSpace(u) == "" {
		return "", errors.New("vunly: the base URL is empty (use WithBaseURL with the API address from the setup step)")
	}
	out := NormalizeBaseURL(u)
	p, err := url.Parse(out)
	if err != nil || (p.Scheme != "http" && p.Scheme != "https") || p.Host == "" {
		return "", errors.New("vunly: the base URL must be an absolute http(s) address such as https://host")
	}
	return out, nil
}

func sleepCtx(ctx context.Context, d time.Duration) error {
	if d <= 0 {
		return ctx.Err()
	}
	t := time.NewTimer(d)
	defer t.Stop()
	select {
	case <-ctx.Done():
		return ctx.Err()
	case <-t.C:
		return nil
	}
}

// ---- requests ---------------------------------------------------------------------------------------------------------

// opClass is the operation class of conformance/README.md.
type opClass byte

const (
	classR opClass = 'R' // reads and the entitlement check
	classK opClass = 'K' // keyed mutations (same id on retry)
	classN opClass = 'N' // naturally idempotent writes: upserts, deletes
	classX opClass = 'X' // everything else: never repeated after an ambiguous failure
)

const (
	maxRetryAfter = 30 * time.Second
	maxBody       = 16 << 20
)

var idempotencyID = regexp.MustCompile(`^[A-Za-z0-9_.:-]{8,128}$`)

// checkID refuses, before any request, an id that could not be an Idempotency-Key: the same error the API would give.
func checkID(id string) error {
	if idempotencyID.MatchString(id) {
		return nil
	}
	return newError(400, "validation_error", "id must be 8 to 128 characters of A-Za-z0-9_.:- (it is sent as the Idempotency-Key)", "",
		map[string]any{"fields": map[string]any{"id": "8-128 chars of A-Za-z0-9_.:-"}}, 0)
}

type request struct {
	method string
	path   string
	query  url.Values
	body   any
	idem   string
	class  opClass
	// shape lists the fields a successful body must carry ("a.b": "string|int|number|bool|array|object", "?" = may be null/absent).
	shape map[string]string
	// out receives the decoded body of a 2xx (nil: the body is discarded).
	out any
}

type meta struct {
	status      int
	requestID   string
	replayed    bool
	planOverage bool
}

func (c *Client) do(ctx context.Context, r request) (meta, error) {
	// A keyed mutation always needs a valid id, so an EMPTY one is refused here too, before any request; for the other
	// classes the id is optional and only checked when present.
	if r.class == classK || r.idem != "" {
		if err := checkID(r.idem); err != nil {
			return meta{}, err
		}
	}
	// An empty, "." or ".." segment in the path (an id that was empty or dot-only) would change the route on the wire
	// (GET /v1/customers/ , GET /v1/): refused before any request.
	for _, seg := range strings.Split(strings.TrimPrefix(r.path, "/"), "/") {
		if seg == "" || seg == "." || seg == ".." {
			return meta{}, newError(400, "validation_error", `an id in the path must not be empty, "." or ".."`, "",
				map[string]any{"fields": map[string]any{"id": `not empty, "." or ".."`}}, 0)
		}
	}
	var payload []byte
	if r.body != nil {
		b, err := json.Marshal(r.body)
		if err != nil {
			return meta{}, fmt.Errorf("vunly: cannot encode the request: %w", err)
		}
		payload = b
	}
	target := c.baseURL + r.path
	if len(r.query) > 0 {
		target += "?" + r.query.Encode()
	}
	for attempt := 1; ; attempt++ {
		m, err := c.once(ctx, r, target, payload)
		if err == nil {
			return m, nil
		}
		var ue *UnavailableError
		if ctx.Err() != nil || !errors.As(err, &ue) || attempt > c.maxRetries || !c.mayRetry(r.class, ue.APIError) {
			return m, err
		}
		wait, ok := retryDelay(attempt, ue.RetryAfter)
		if !ok {
			return m, err
		}
		if err := c.sleep(ctx, wait); err != nil {
			return m, ctxError(err)
		}
	}
}

// mayRetry applies the retry classes: R, K and N are repeated after a lost answer, a timeout or a 5xx; X only after a
// 429, which was refused before it ran. The plan-volume stop is never repeated.
func (c *Client) mayRetry(class opClass, e *APIError) bool {
	switch e.Status {
	case 0, 429, 500, 502, 503, 504:
	default:
		return false // invalid_response and anything else: the status is not a retryable one
	}
	if isPlanStop(e) {
		return false
	}
	if class == classX && e.Status != 429 {
		return false
	}
	return true
}

func retryDelay(attempt int, retryAfter time.Duration) (time.Duration, bool) {
	if retryAfter > 0 {
		return retryAfter, retryAfter <= maxRetryAfter
	}
	base := 250 * time.Millisecond << (attempt - 1)
	if base > 8*time.Second {
		base = 8 * time.Second
	}
	return time.Duration(float64(base) * (0.5 + rand.Float64()/2)), true
}

// ctxError says what the caller's context ending means. Cancelling is the caller's own decision and stays the context's
// error (errors.Is(err, context.Canceled)). A deadline that expired before the API answered is "no answer in time": an
// *UnavailableError, so `catch Unavailable` and the fail-closed branch see it like any other timeout, that still wraps
// context.DeadlineExceeded for errors.Is.
func ctxError(err error) error {
	if errors.Is(err, context.DeadlineExceeded) {
		return &UnavailableError{&APIError{
			Code:    "unavailable",
			Message: "the context deadline expired before the Vunly API answered",
			cause:   err,
		}}
	}
	return err
}

func (c *Client) once(ctx context.Context, r request, target string, payload []byte) (meta, error) {
	actx, cancel := context.WithTimeout(ctx, c.timeout)
	defer cancel()
	var body io.Reader
	if payload != nil {
		body = bytes.NewReader(payload)
	}
	req, err := http.NewRequestWithContext(actx, r.method, target, body)
	if err != nil {
		return meta{}, fmt.Errorf("vunly: cannot build the request: %w", err)
	}
	// net/http treats a request with an Idempotency-Key and a replayable body as idempotent and silently sends it again
	// when a reused connection drops. That would make a second attempt the SDK never counted, with retries off and for
	// classes the contract forbids repeating. Without GetBody the transport cannot replay it; only this client retries.
	req.GetBody = nil
	req.Header.Set("Authorization", "Bearer "+string(c.apiKey))
	req.Header.Set("User-Agent", c.userAgent)
	req.Header.Set("Accept", "application/json")
	if payload != nil {
		req.Header.Set("Content-Type", "application/json")
	}
	if r.idem != "" {
		req.Header.Set("Idempotency-Key", r.idem)
	}
	res, err := c.http.Do(req)
	if err != nil {
		if ctx.Err() != nil {
			return meta{}, ctxError(ctx.Err())
		}
		// Deliberately drop the underlying error: some transports put the URL or headers in it.
		msg := "could not reach the Vunly API"
		if errors.Is(err, context.DeadlineExceeded) {
			msg = fmt.Sprintf("the Vunly API did not answer within %s", c.timeout)
		}
		return meta{}, newError(0, "unavailable", msg, "", nil, 0)
	}
	defer res.Body.Close()
	raw, rerr := io.ReadAll(io.LimitReader(res.Body, maxBody+1))
	m := meta{
		status:      res.StatusCode,
		requestID:   cleanText(res.Header.Get("Request-Id"), maxRequestID, string(c.apiKey)),
		replayed:    res.Header.Get("Idempotent-Replayed") == "true",
		planOverage: res.Header.Get("Vunly-Plan-Overage") == "true",
	}
	if res.StatusCode >= 200 && res.StatusCode < 300 {
		if rerr != nil || len(raw) > maxBody {
			return m, invalidResponse(res.StatusCode, m.requestID, "the body could not be read")
		}
		return m, decodeSuccess(res.StatusCode, m.requestID, raw, r)
	}
	return m, errorFromResponse(res, m.requestID, raw, string(c.apiKey))
}

// maxParsedRetryAfter bounds what a Retry-After can say. The value is converted to a time.Duration (int64 nanoseconds,
// about 292 years), and a hostile or broken server can send 99999999999, 1e12 or Inf; an overflow would wrap negative and
// look like "no wait at all". Anything above this is reported as this much: still far over the 30 s the SDK will wait,
// so it is surfaced to the caller and never slept on. One day, as in every SDK (conformance/README.md).
const maxParsedRetryAfter = 24 * time.Hour

// retryAfterNumber is the whole syntax of a Retry-After this API's SDKs honour: ASCII digits, an optional fraction, an optional
// exponent. strconv.ParseFloat alone would also accept "0x1p3", "inf", "nan" and underscores.
var retryAfterNumber = regexp.MustCompile(`(?i)^(\+?([0-9]+(\.[0-9]*)?|\.[0-9]+)(e[+-]?[0-9]+)?|\+?inf(inity)?)$`)

// parseRetryAfter reads the delta-seconds form. Anything that is not such a number (an HTTP date, garbage, a sign) means "none".
// A number too large for a float64 ("1e999", hundreds of digits) is NOT "none": ParseFloat reports it as +Inf with ErrRange,
// and it is a very long wait, so it is capped like any other large one and surfaced, never retried early.
func parseRetryAfter(v string) time.Duration {
	v = strings.TrimSpace(v)
	if !retryAfterNumber.MatchString(v) {
		return 0
	}
	secs, err := strconv.ParseFloat(v, 64)
	if err != nil {
		var ne *strconv.NumError
		if !errors.As(err, &ne) || ne.Err != strconv.ErrRange || !math.IsInf(secs, 1) {
			return 0 // an underflow ("1e-999") rounds to 0, which is no directive
		}
		return maxParsedRetryAfter
	}
	if math.IsNaN(secs) || secs <= 0 {
		return 0
	}
	if secs >= maxParsedRetryAfter.Seconds() {
		return maxParsedRetryAfter
	}
	return time.Duration(secs * float64(time.Second))
}

func errorFromResponse(res *http.Response, requestID string, raw []byte, key string) error {
	if res.StatusCode >= 300 && res.StatusCode < 400 {
		// The SDK never follows a redirect (it would send your key to the new address) and never retries one, so this
		// is a configuration problem, not a bad request, whatever the body says (a body is not trusted here).
		return newError(res.StatusCode, "unexpected_redirect",
			fmt.Sprintf("the API address answered a redirect (%d); the SDK never follows redirects, because that would send your key to another address: check the base URL", res.StatusCode),
			requestID, nil, 0)
	}
	var env struct {
		Error struct {
			Code      string         `json:"code"`
			Message   string         `json:"message"`
			RequestID string         `json:"requestId"`
			Details   map[string]any `json:"details"`
		} `json:"error"`
	}
	_ = json.Unmarshal(raw, &env) // an error with no body (a proxy, an empty 5xx) is still classified by its status
	e := env.Error
	e.Code = cleanText(e.Code, maxCode, key)
	e.Message = cleanText(e.Message, maxMessage, key)
	e.RequestID = cleanText(e.RequestID, maxRequestID, key)
	var details map[string]any
	if len(e.Details) > 0 {
		details, _ = cleanDetails(e.Details, 0, key).(map[string]any)
	}
	e.Details = details
	if e.Code == "" {
		e.Code = fallbackCode(res.StatusCode)
	}
	if e.Message == "" {
		e.Message = "request failed with status " + strconv.Itoa(res.StatusCode)
	}
	if res.StatusCode == 404 && strings.Contains(strings.ToLower(e.Message), "no such route") {
		e.Message += " (check the API address you configured: it should be the address only, for example https://host; a trailing /v1 is accepted)"
	}
	if e.RequestID != "" {
		requestID = e.RequestID
	}
	retryAfter := parseRetryAfter(res.Header.Get("Retry-After"))
	return newError(res.StatusCode, e.Code, e.Message, requestID, e.Details, retryAfter)
}

// decodeSuccess checks that a 2xx carries the fields the SDK promises and decodes it. A body that does not is
// invalid_response (Unavailable): fail closed, never a zero value that looks like an answer.
func decodeSuccess(status int, requestID string, raw []byte, r request) error {
	if r.out == nil && len(r.shape) == 0 {
		return nil
	}
	var generic any
	dec := json.NewDecoder(bytes.NewReader(raw))
	dec.UseNumber()
	if err := dec.Decode(&generic); err != nil {
		return invalidResponse(status, requestID, "the body is not JSON")
	}
	if _, ok := generic.(map[string]any); !ok {
		return invalidResponse(status, requestID, "the body is not a JSON object")
	}
	if problem := shapeProblem(generic, r.shape); problem != "" {
		return invalidResponse(status, requestID, problem)
	}
	if r.out != nil {
		if err := json.Unmarshal(raw, r.out); err != nil {
			return invalidResponse(status, requestID, "unexpected shape")
		}
	}
	return nil
}

func shapeProblem(body any, shape map[string]string) string {
	for path, spec := range shape {
		nullable := strings.HasSuffix(spec, "?")
		typ := strings.TrimSuffix(spec, "?")
		var v any = body
		for _, part := range strings.Split(path, ".") {
			m, ok := v.(map[string]any)
			if !ok {
				v = nil
				break
			}
			v = m[part]
		}
		if v == nil {
			if nullable {
				continue
			}
			return fmt.Sprintf("field %q is missing", path)
		}
		good := false
		switch typ {
		case "string":
			_, good = v.(string)
		case "bool":
			_, good = v.(bool)
		case "array":
			_, good = v.([]any)
		case "object":
			_, good = v.(map[string]any)
		case "int":
			if n, ok := v.(json.Number); ok {
				_, err := n.Int64()
				good = err == nil
			}
		}
		if !good {
			return fmt.Sprintf("field %q is not %s", path, typ)
		}
	}
	return ""
}
