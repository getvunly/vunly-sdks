package vunly

import (
	"fmt"
	"time"
)

// Customer is a customer you bill. ID is Vunly's own id (cus_...); you never have to store it: every call accepts your
// ExternalID instead.
type Customer struct {
	ID         string    `json:"id"`
	ExternalID string    `json:"externalId"`
	Name       string    `json:"name"` // empty when none was given
	CreatedAt  time.Time `json:"createdAt"`
}

// LedgerEntry is one line of a customer's ledger. Credits is signed: positive for a grant, negative for a debit.
type LedgerEntry struct {
	ID             string    `json:"id"`
	CustomerID     string    `json:"customerId"`
	Kind           string    `json:"kind"` // "grant" or "debit"
	Credits        int64     `json:"credits"`
	Meter          string    `json:"meter"`    // empty for a grant
	Quantity       int64     `json:"quantity"` // 0 for a grant
	Reason         string    `json:"reason"`   // empty when none was given
	IdempotencyKey string    `json:"idempotencyKey"`
	CreatedAt      time.Time `json:"createdAt"`
}

// UsageResult is the outcome of Usage.Record.
type UsageResult struct {
	Event LedgerEntry
	// Remaining is the credits left after this event.
	Remaining int64
	// Replayed is true when this ID had already been recorded: nothing was debited again. A replay is a success.
	Replayed bool
	// PlanOverage is true when the project is on a paid plan and this event is past the plan's included monthly volume.
	// Paid plans keep recording; this lets you notice. Always false for a replay.
	PlanOverage bool
	// RequestID is the API's Request-Id.
	RequestID string
}

// Balance is a customer's credits. Remaining = Granted - Consumed and is never negative.
type Balance struct {
	Remaining int64 `json:"credits"`
	Consumed  int64 `json:"consumed"`
	Granted   int64 `json:"granted"`
}

// Estimate is the usage a check should price in.
type Estimate struct {
	Meter    string `json:"meter"`
	Quantity int64  `json:"quantity"`
}

// Entitlement is the answer of Entitlements.Check. It is advisory and reserves nothing: the authoritative limit is the
// debit made by Usage.Record. Unless Allowed is true, do not run the work.
type Entitlement struct {
	Allowed bool `json:"allowed"`
	// Reason is "ok", "no_active_plan", "feature_not_in_plan" or "insufficient_credits".
	Reason    string `json:"reason"`
	Remaining int64  `json:"remaining"`
	Required  int64  `json:"required"`
	Plan      string `json:"plan"` // empty when the customer has none
}

// PlanRef names a plan.
type PlanRef struct {
	Key  string `json:"key"`
	Name string `json:"name"`
}

// Plan is a plan of the project.
type Plan struct {
	Key             string    `json:"key"`
	Name            string    `json:"name"`
	IncludedCredits int64     `json:"includedCredits"`
	Meter           string    `json:"meter"`
	Features        []string  `json:"features"`
	UpdatedAt       time.Time `json:"updatedAt"`
}

// Subscription is a customer's plan. Status is "active" or "canceled".
type Subscription struct {
	ID          string    `json:"id"`
	CustomerID  string    `json:"customerId"`
	Plan        Plan      `json:"plan"`
	Status      string    `json:"status"`
	PeriodStart time.Time `json:"periodStart"`
	PeriodEnd   time.Time `json:"periodEnd"`
	CreatedAt   time.Time `json:"createdAt"`
}

// CustomerToken is a short-lived read token for ONE customer, for the usage widget in a browser. It is shown once; a
// browser may hold it, never an API key. Printing it shows only its id.
type CustomerToken struct {
	ID         string    `json:"id"`
	Token      string    `json:"token"`
	CustomerID string    `json:"customerId"`
	ExpiresAt  time.Time `json:"expiresAt"`
}

func (t CustomerToken) String() string {
	return fmt.Sprintf("CustomerToken{ID: %q, CustomerID: %q, ExpiresAt: %s, Token: <hidden>}", t.ID, t.CustomerID, t.ExpiresAt.Format(time.RFC3339))
}

// GoString keeps %#v from printing the token.
func (t CustomerToken) GoString() string { return t.String() }

// PlanUsage is the project's plan and this month's recorded usage.
//
// UnconfirmedLimits is a working hypothesis, not approved and not a guarantee: it carries Confirmed == false. Do not
// show it to your customers as a quota, a price or a commitment.
type PlanUsage struct {
	Plan   PlanRef `json:"plan"`
	Period struct {
		Start time.Time `json:"start"`
		End   time.Time `json:"end"`
	} `json:"period"`
	Usage struct {
		Events int64 `json:"events"`
	} `json:"usage"`
	UnconfirmedLimits struct {
		Confirmed             bool   `json:"confirmed"`
		Note                  string `json:"note"`
		IncludedMonthlyEvents *int64 `json:"includedMonthlyEvents"`
		ActivityRetentionDays *int64 `json:"activityRetentionDays"`
	} `json:"unconfirmedLimits"`
}

// Page is one page of a list. NextCursor is opaque: pass it back as ListOptions.Cursor. It is empty on the last page.
type Page[T any] struct {
	Data       []T
	NextCursor string
}

// HasMore reports whether another page follows.
func (p *Page[T]) HasMore() bool { return p.NextCursor != "" }

// ListOptions pages a list. Limit is 1 to 100 (0 means the API's default, 25).
type ListOptions struct {
	Limit  int
	Cursor string
}
