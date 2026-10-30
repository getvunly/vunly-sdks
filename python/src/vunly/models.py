"""Result types. Plain frozen dataclasses; every result that comes from a request carries ``request_id``."""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any, Mapping


@dataclass(frozen=True)
class Customer:
    id: str
    external_id: str
    name: str | None
    created_at: str
    request_id: str | None = None


@dataclass(frozen=True)
class LedgerEntry:
    id: str
    customer_id: str
    kind: str  # "grant" | "debit"
    credits: int  # signed: positive for a grant, negative for a debit
    meter: str | None
    quantity: int | None
    reason: str | None
    idempotency_key: str | None
    created_at: str
    request_id: str | None = None


@dataclass(frozen=True)
class UsageResult:
    event: LedgerEntry
    remaining: int
    #: True when this id had already been recorded: nothing was debited again.
    replayed: bool
    request_id: str | None = None
    #: True when the project is on a paid plan and this event is past the plan's included monthly volume. Paid plans keep
    #: recording; this lets you notice. Always False for a replay.
    plan_overage: bool = False


@dataclass(frozen=True)
class Page:
    """One page of a list. ``next_cursor`` is opaque: pass it back as ``cursor``; ``None`` means the end."""

    data: list[Any]
    next_cursor: str | None
    request_id: str | None = None


@dataclass(frozen=True)
class PlanUsage:
    """The project's plan and this month's recorded usage.

    ``unconfirmed_limits`` (included monthly events, activity retention days) is a working hypothesis: it carries
    ``confirmed: false`` and is not a guarantee, a quota or a price. Do not show it to your customers as a commitment.
    """

    plan: Mapping[str, Any]
    period: Mapping[str, Any]
    usage: Mapping[str, Any]
    unconfirmed_limits: Mapping[str, Any]
    request_id: str | None = None


@dataclass(frozen=True)
class Balance:
    remaining: int
    consumed: int
    granted: int
    request_id: str | None = None


@dataclass(frozen=True)
class Entitlement:
    allowed: bool
    #: "ok", "no_active_plan", "feature_not_in_plan" or "insufficient_credits". Anything but "ok": do not run the work.
    reason: str
    remaining: int
    required: int
    plan: str | None
    request_id: str | None = None


@dataclass(frozen=True)
class CustomerToken:
    id: str
    token: str  # shown once; a browser may hold it, never an API key
    customer_id: str
    expires_at: str
    request_id: str | None = None

    def __repr__(self) -> str:  # never print the secret
        return f"CustomerToken(id={self.id!r}, customer_id={self.customer_id!r}, expires_at={self.expires_at!r}, token=<hidden>)"


@dataclass(frozen=True)
class Raw:
    """A response the SDK does not model yet (plans, subscriptions, usage summaries): the decoded JSON as ``data``."""

    data: Mapping[str, Any] = field(default_factory=dict)
    request_id: str | None = None


def _g(d: Mapping[str, Any], key: str, default: Any = None) -> Any:
    return d.get(key, default)


def ledger_entry(d: Mapping[str, Any], request_id: str | None) -> LedgerEntry:
    return LedgerEntry(
        id=d["id"],
        customer_id=d["customerId"],
        kind=d["kind"],
        credits=d["credits"],
        meter=_g(d, "meter"),
        quantity=_g(d, "quantity"),
        reason=_g(d, "reason"),
        idempotency_key=_g(d, "idempotencyKey"),
        created_at=d["createdAt"],
        request_id=request_id,
    )
