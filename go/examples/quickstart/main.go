// Command quickstart records one unit of work for one customer against a Vunly sandbox API.
//
//	VUNLY_API_URL=http://127.0.0.1:8787 VUNLY_API_KEY=vk_test_... VUNLY_PLAN=starter go run .
//
// The sandbox project needs a meter called "tokens" and a plan (VUNLY_PLAN) whose features include "generate"; the key
// needs the scopes the onboarding wizard issues.
package main

import (
	"context"
	"errors"
	"fmt"
	"os"
	"time"

	vunly "github.com/getvunly/vunly-sdks/go"
)

func main() {
	if err := run(); err != nil {
		fmt.Fprintln(os.Stderr, "quickstart:", err)
		os.Exit(1)
	}
}

func run() error {
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()

	// The key comes from the environment. It is never printed: a Client prints as vunly.Client{baseURL, apiKey: <hidden>}.
	client, err := vunly.New(os.Getenv("VUNLY_API_KEY"), vunly.WithBaseURL(os.Getenv("VUNLY_API_URL")))
	if err != nil {
		return err
	}

	customer := fmt.Sprintf("cus_quickstart_%d", time.Now().Unix())
	if _, err := client.Customers.Upsert(ctx, customer, &vunly.CustomerParams{Name: "Quickstart"}); err != nil {
		return err
	}
	if _, err := client.Credits.Grant(ctx, vunly.GrantParams{ID: "grant_" + customer, Customer: customer, Credits: 100, Reason: "sandbox top-up"}); err != nil {
		return err
	}

	// Put the customer on a plan when one is named (the plan itself is created with an admin key, not with this one).
	if plan := os.Getenv("VUNLY_PLAN"); plan != "" {
		if _, err := client.Plans.Subscribe(ctx, customer, plan); err != nil {
			return err
		}
	}

	// Ask first. A failed check is a "no", and so is an error: fail closed, do not run the work.
	decision, err := client.Entitlements.Check(ctx, vunly.CheckParams{
		Customer: customer, Feature: "generate",
		Estimate: &vunly.Estimate{Meter: "tokens", Quantity: 1050},
	})
	if err != nil {
		return err
	}
	fmt.Printf("check: allowed=%v reason=%s remaining=%d\n", decision.Allowed, decision.Reason, decision.Remaining)
	if !decision.Allowed {
		return fmt.Errorf("not allowed (%s); set VUNLY_PLAN to a plan with the feature \"generate\"", decision.Reason)
	}

	// The work happens here. Then record it once, under an ID that names this unit of work.
	id := "req_" + customer
	res, err := client.Usage.Record(ctx, vunly.RecordParams{ID: id, Customer: customer, Meter: "tokens", Quantity: 1050})
	var limit *vunly.LimitReachedError
	switch {
	case errors.As(err, &limit):
		balance, _ := limit.Balance()
		return fmt.Errorf("not enough credits (balance %d); nothing was debited", balance)
	case err != nil:
		return err
	}
	fmt.Printf("recorded: remaining=%d replayed=%v request=%s\n", res.Remaining, res.Replayed, res.RequestID)

	// Recording the same ID again is safe: nothing is debited twice.
	again, err := client.Usage.Record(ctx, vunly.RecordParams{ID: id, Customer: customer, Meter: "tokens", Quantity: 1050})
	if err != nil {
		return err
	}
	fmt.Printf("again: remaining=%d replayed=%v\n", again.Remaining, again.Replayed)
	return nil
}
