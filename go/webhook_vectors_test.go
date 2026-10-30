package vunly_test

import (
	"encoding/json"
	"errors"
	"os"
	"testing"
	"time"

	vunly "github.com/getvunly/vunly-sdks/go"
)

// The shared webhook vectors (testdata/webhook-vectors.json, an identical copy in every SDK). The signatures were computed
// outside any SDK. "expect" is "ok", a WebhookError reason, or "config" (ErrEmptyWebhookSecret, raised before the signature
// is looked at). A null header is the empty string here: Go has no absent string, and the SDK treats both as missing_header.
func TestWebhookSharedVectors(t *testing.T) {
	raw, err := os.ReadFile("testdata/webhook-vectors.json")
	if err != nil {
		t.Fatal(err)
	}
	var file struct {
		Now     int64 `json:"now"`
		Vectors []struct {
			Name    string   `json:"name"`
			Payload string   `json:"payload"`
			Header  *string  `json:"header"`
			Secrets []string `json:"secrets"`
			Expect  string   `json:"expect"`
		} `json:"vectors"`
	}
	if err := json.Unmarshal(raw, &file); err != nil {
		t.Fatal(err)
	}
	if len(file.Vectors) < 20 {
		t.Fatalf("only %d vectors", len(file.Vectors))
	}
	now := time.Unix(file.Now, 0)
	for _, v := range file.Vectors {
		t.Run(v.Name, func(t *testing.T) {
			header := ""
			if v.Header != nil {
				header = *v.Header
			}
			_, err := vunly.VerifyWebhook([]byte(v.Payload), header, v.Secrets, vunly.WithWebhookClock(func() time.Time { return now }))
			got := "ok"
			var we *vunly.WebhookError
			switch {
			case err == nil:
			case errors.Is(err, vunly.ErrEmptyWebhookSecret):
				got = "config"
			case errors.As(err, &we):
				got = we.Reason
			default:
				t.Fatalf("unexpected error %v", err)
			}
			if got != v.Expect {
				t.Fatalf("got %s, want %s", got, v.Expect)
			}
		})
	}
}
