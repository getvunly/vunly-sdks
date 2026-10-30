package vunly

import (
	"context"
	"iter"
	"net/url"
	"strconv"
	"time"
)

func esc(s string) string { return url.PathEscape(s) }

func pageQuery(o *ListOptions) url.Values {
	q := url.Values{}
	if o != nil {
		if o.Limit != 0 {
			q.Set("limit", strconv.Itoa(o.Limit))
		}
		if o.Cursor != "" {
			q.Set("cursor", o.Cursor)
		}
	}
	return q
}

var customerShape = map[string]string{"id": "string", "externalId": "string", "createdAt": "string"}
var entryShape = map[string]string{"id": "string", "customerId": "string", "kind": "string", "credits": "int", "createdAt": "string"}
var listShape = map[string]string{"data": "array", "nextCursor": "string?"}

// ---- customers ------------------------------------------------------------------------------------------------------

// CustomersService creates and reads the customers you bill.
type CustomersService struct{ c *Client }

// CustomerParams are the optional fields of Upsert.
type CustomerParams struct {
	// Name is shown in the console; empty leaves it unset.
	Name string
}

// Upsert creates the customer or returns the existing one for this id (your own id). Asking twice leaves one customer.
func (s *CustomersService) Upsert(ctx context.Context, id string, p *CustomerParams) (*Customer, error) {
	body := map[string]any{"externalId": id}
	if p != nil && p.Name != "" {
		body["name"] = p.Name
	}
	var out Customer
	if _, err := s.c.do(ctx, request{method: "POST", path: "/v1/customers", body: body, class: classN, shape: customerShape, out: &out}); err != nil {
		return nil, err
	}
	return &out, nil
}

// Get returns one customer, by your id or Vunly's. A customer that does not exist is an *InvalidRequestError with code
// "not_found".
func (s *CustomersService) Get(ctx context.Context, customer string) (*Customer, error) {
	var out Customer
	if _, err := s.c.do(ctx, request{method: "GET", path: "/v1/customers/" + esc(customer), class: classR, shape: customerShape, out: &out}); err != nil {
		return nil, err
	}
	return &out, nil
}

// List returns one page of customers, newest first.
func (s *CustomersService) List(ctx context.Context, o *ListOptions) (*Page[Customer], error) {
	var raw struct {
		Data       []Customer `json:"data"`
		NextCursor string     `json:"nextCursor"`
	}
	if _, err := s.c.do(ctx, request{method: "GET", path: "/v1/customers", query: pageQuery(o), class: classR, shape: listShape, out: &raw}); err != nil {
		return nil, err
	}
	return &Page[Customer]{Data: raw.Data, NextCursor: raw.NextCursor}, nil
}

// All iterates over every customer, fetching one page at a time and only as you consume them. Stop ranging to stop fetching.
func (s *CustomersService) All(ctx context.Context, limit int) iter.Seq2[Customer, error] {
	return paged(func(cursor string) (*Page[Customer], error) {
		return s.List(ctx, &ListOptions{Limit: limit, Cursor: cursor})
	})
}

// ---- credits --------------------------------------------------------------------------------------------------------

// CreditsService grants credits and reads balances and the ledger.
type CreditsService struct{ c *Client }

// GrantParams are the fields of Grant. ID is required: it is the Idempotency-Key, the same ID grants once.
type GrantParams struct {
	ID       string // 8 to 128 characters of A-Za-z0-9_.:-
	Customer string
	Credits  int64
	Reason   string // optional
}

// Grant adds credits (a sandbox top-up). Repeating the same ID grants once and returns the original entry.
func (s *CreditsService) Grant(ctx context.Context, p GrantParams) (*LedgerEntry, error) {
	body := map[string]any{"credits": p.Credits}
	if p.Reason != "" {
		body["reason"] = p.Reason
	}
	var out LedgerEntry
	if _, err := s.c.do(ctx, request{method: "POST", path: "/v1/customers/" + esc(p.Customer) + "/credit-grants", body: body, idem: p.ID, class: classK, shape: entryShape, out: &out}); err != nil {
		return nil, err
	}
	return &out, nil
}

// Balance returns a customer's credits.
func (s *CreditsService) Balance(ctx context.Context, customer string) (*Balance, error) {
	var out Balance
	shape := map[string]string{"credits": "int", "consumed": "int", "granted": "int"}
	if _, err := s.c.do(ctx, request{method: "GET", path: "/v1/customers/" + esc(customer) + "/balance", class: classR, shape: shape, out: &out}); err != nil {
		return nil, err
	}
	return &out, nil
}

// Ledger returns one page of a customer's ledger entries, newest first.
func (s *CreditsService) Ledger(ctx context.Context, customer string, o *ListOptions) (*Page[LedgerEntry], error) {
	var raw struct {
		Data       []LedgerEntry `json:"data"`
		NextCursor string        `json:"nextCursor"`
	}
	if _, err := s.c.do(ctx, request{method: "GET", path: "/v1/customers/" + esc(customer) + "/ledger", query: pageQuery(o), class: classR, shape: listShape, out: &raw}); err != nil {
		return nil, err
	}
	return &Page[LedgerEntry]{Data: raw.Data, NextCursor: raw.NextCursor}, nil
}

// AllLedger iterates over a customer's whole ledger, one page at a time, only as you consume it.
func (s *CreditsService) AllLedger(ctx context.Context, customer string, limit int) iter.Seq2[LedgerEntry, error] {
	return paged(func(cursor string) (*Page[LedgerEntry], error) {
		return s.Ledger(ctx, customer, &ListOptions{Limit: limit, Cursor: cursor})
	})
}

const maxEmptyPagesInARow = 3

// paged turns a page fetcher into an iterator that fetches lazily and ends after the first error.
func paged[T any](fetch func(cursor string) (*Page[T], error)) iter.Seq2[T, error] {
	return func(yield func(T, error) bool) {
		cursor := ""
		seen := map[string]bool{}
		emptyInARow := 0
		for {
			p, err := fetch(cursor)
			if err != nil {
				var zero T
				yield(zero, err)
				return
			}
			for _, v := range p.Data {
				if !yield(v, nil) {
					return
				}
			}
			if !p.HasMore() {
				return
			}
			// A server that hands back a cursor already used, or keeps promising more with empty pages, must not make the
			// iteration endless: fail closed like any other answer that cannot be right.
			if len(p.Data) == 0 {
				emptyInARow++
			} else {
				emptyInARow = 0
			}
			if seen[p.NextCursor] || emptyInARow >= maxEmptyPagesInARow {
				var zero T
				yield(zero, invalidResponse(200, "", "the next-page cursor did not advance or empty pages keep coming"))
				return
			}
			seen[p.NextCursor] = true
			cursor = p.NextCursor
		}
	}
}

// ---- usage ----------------------------------------------------------------------------------------------------------

// UsageService records usage events.
type UsageService struct{ c *Client }

// RecordParams are the fields of Record. ID is required and is never generated for you.
type RecordParams struct {
	// ID names the unit of work (your request or job id) and is sent as the Idempotency-Key: 8 to 128 characters of
	// A-Za-z0-9_.:-. Retry with the SAME ID; the API applies it at most once.
	ID       string
	Customer string
	Meter    string
	// Quantity is in the meter's own units (for example tokens), not credits: the project's meter converts it, rounded
	// up per request.
	Quantity int64
}

// Record records usage once and debits credits. It is final: a *LimitReachedError means the credits were not there when
// you recorded, even if a check said yes earlier; nothing was debited. A repeat of the same ID returns the original
// result with Replayed == true.
func (s *UsageService) Record(ctx context.Context, p RecordParams) (*UsageResult, error) {
	var raw struct {
		Event   LedgerEntry `json:"event"`
		Balance int64       `json:"balance"`
	}
	shape := map[string]string{"balance": "int", "event.id": "string", "event.customerId": "string", "event.kind": "string", "event.credits": "int", "event.createdAt": "string"}
	m, err := s.c.do(ctx, request{
		method: "POST", path: "/v1/usage-events",
		body: map[string]any{"customerId": p.Customer, "meter": p.Meter, "quantity": p.Quantity},
		idem: p.ID, class: classK, shape: shape, out: &raw,
	})
	if err != nil {
		return nil, err
	}
	return &UsageResult{Event: raw.Event, Remaining: raw.Balance, Replayed: m.replayed, PlanOverage: m.planOverage && !m.replayed, RequestID: m.requestID}, nil
}

// ---- entitlements ---------------------------------------------------------------------------------------------------

// EntitlementsService asks whether a customer may do something now.
type EntitlementsService struct{ c *Client }

// CheckParams are the fields of Check.
type CheckParams struct {
	Customer string
	Feature  string
	// Estimate is optional: the usage the work is expected to need.
	Estimate *Estimate
}

// Check is read-only and advisory. If it returns an error (a *UnavailableError after retries), treat that as "no": fail
// closed, do not run the work. It never returns Allowed == true by itself.
func (s *EntitlementsService) Check(ctx context.Context, p CheckParams) (*Entitlement, error) {
	body := map[string]any{"customerId": p.Customer, "feature": p.Feature}
	if p.Estimate != nil {
		body["estimate"] = p.Estimate
	}
	var out Entitlement
	shape := map[string]string{"allowed": "bool", "reason": "string", "remaining": "int", "required": "int", "plan": "string?"}
	if _, err := s.c.do(ctx, request{method: "POST", path: "/v1/entitlements/check", body: body, class: classR, shape: shape, out: &out}); err != nil {
		return nil, err
	}
	return &out, nil
}

// ---- plans ----------------------------------------------------------------------------------------------------------

// PlansService manages meters, plans and subscriptions.
type PlansService struct{ c *Client }

// UpsertMeter creates or updates a meter: how many units of usage cost one credit.
func (s *PlansService) UpsertMeter(ctx context.Context, key string, unitsPerCredit int64) error {
	_, err := s.c.do(ctx, request{method: "POST", path: "/v1/meters", body: map[string]any{"key": key, "unitsPerCredit": unitsPerCredit}, class: classN})
	return err
}

// PlanParams are the fields of UpsertPlan.
type PlanParams struct {
	Key             string
	Name            string
	IncludedCredits int64
	Meter           string
	Features        []string
}

// UpsertPlan creates or updates a plan.
func (s *PlansService) UpsertPlan(ctx context.Context, p PlanParams) (*Plan, error) {
	features := p.Features
	if features == nil {
		features = []string{}
	}
	body := map[string]any{"key": p.Key, "name": p.Name, "includedCredits": p.IncludedCredits, "meter": p.Meter, "features": features}
	var out Plan
	shape := map[string]string{"key": "string", "name": "string", "includedCredits": "int", "meter": "string", "features": "array"}
	if _, err := s.c.do(ctx, request{method: "POST", path: "/v1/plans", body: body, class: classN, shape: shape, out: &out}); err != nil {
		return nil, err
	}
	return &out, nil
}

// Subscribe activates a plan for the customer. The same plan again returns the existing subscription and grants nothing
// more; another plan while one is active is an *InvalidRequestError with code "subscription_exists".
func (s *PlansService) Subscribe(ctx context.Context, customer, plan string) (*Subscription, error) {
	var out Subscription
	shape := map[string]string{"id": "string", "customerId": "string", "status": "string", "plan.key": "string"}
	if _, err := s.c.do(ctx, request{method: "POST", path: "/v1/customers/" + esc(customer) + "/subscription", body: map[string]any{"plan": plan}, class: classN, shape: shape, out: &out}); err != nil {
		return nil, err
	}
	return &out, nil
}

// Cancel cancels the customer's active subscription.
func (s *PlansService) Cancel(ctx context.Context, customer string) error {
	_, err := s.c.do(ctx, request{method: "DELETE", path: "/v1/customers/" + esc(customer) + "/subscription", class: classN})
	return err
}

// ---- customer tokens ------------------------------------------------------------------------------------------------

// CustomerTokensService mints short-lived read tokens for one customer, for a browser.
type CustomerTokensService struct{ c *Client }

// Create mints a token for ONE customer. ttl is 30 seconds to 1 hour (0 means 15 minutes). Every call mints a new
// token and has no idempotency key, so after a lost answer the SDK does NOT repeat it: it returns an
// *UnavailableError and you decide.
func (s *CustomerTokensService) Create(ctx context.Context, customer string, ttl time.Duration) (*CustomerToken, error) {
	body := map[string]any{"customerId": customer}
	if ttl > 0 {
		body["ttlSeconds"] = int(ttl / time.Second)
	}
	var out CustomerToken
	shape := map[string]string{"id": "string", "token": "string", "customerId": "string", "expiresAt": "string"}
	if _, err := s.c.do(ctx, request{method: "POST", path: "/v1/customer-tokens", body: body, class: classX, shape: shape, out: &out}); err != nil {
		return nil, err
	}
	return &out, nil
}

// Revoke revokes a token by its id. Idempotent.
func (s *CustomerTokensService) Revoke(ctx context.Context, id string) error {
	_, err := s.c.do(ctx, request{method: "DELETE", path: "/v1/customer-tokens/" + esc(id), class: classN})
	return err
}

// ---- plan usage -----------------------------------------------------------------------------------------------------

// PlanUsage returns the project's plan and this month's recorded usage. Its limits are inside UnconfirmedLimits and are
// not a guarantee (Confirmed is false).
func (c *Client) PlanUsage(ctx context.Context) (*PlanUsage, error) {
	var out PlanUsage
	shape := map[string]string{"plan.key": "string", "period.start": "string", "usage.events": "int", "unconfirmedLimits.confirmed": "bool"}
	if _, err := c.do(ctx, request{method: "GET", path: "/v1/plan-usage", class: classR, shape: shape, out: &out}); err != nil {
		return nil, err
	}
	return &out, nil
}
