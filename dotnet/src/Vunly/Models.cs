using System;
using System.Collections.Generic;

namespace Vunly;

/// <summary>
/// A customer you bill. <c>Id</c> is Vunly's own id (cus_...); you never have to store it: every call accepts your
/// <c>ExternalId</c> instead.
/// </summary>
/// <param name="Name">Null when none was given.</param>
public sealed record Customer(string Id, string ExternalId, string? Name, DateTimeOffset CreatedAt)
{
    internal static Customer From(Fields f) => new(f.Str("id"), f.Str("externalId"), f.StrOrNull("name"), f.Time("createdAt"));
}

/// <summary>One line of a customer's ledger.</summary>
/// <param name="Kind">"grant" or "debit".</param>
/// <param name="Credits">Signed: positive for a grant, negative for a debit.</param>
/// <param name="Meter">Null for a grant.</param>
/// <param name="Quantity">0 for a grant.</param>
/// <param name="Reason">Null when none was given.</param>
public sealed record LedgerEntry(
    string Id,
    string CustomerId,
    string Kind,
    long Credits,
    string? Meter,
    long Quantity,
    string? Reason,
    string? IdempotencyKey,
    DateTimeOffset CreatedAt)
{
    internal static LedgerEntry From(Fields f) =>
        new(
            f.Str("id"),
            f.Str("customerId"),
            f.Str("kind"),
            f.Long("credits"),
            f.StrOrNull("meter"),
            f.LongOr("quantity", 0),
            f.StrOrNull("reason"),
            f.StrOrNull("idempotencyKey"),
            f.Time("createdAt"));
}

/// <summary>The outcome of <see cref="UsageApi.RecordAsync"/>.</summary>
/// <param name="Remaining">The credits left after this event.</param>
/// <param name="Replayed">
/// True when this id had already been recorded: nothing was debited again. A replay is a success.
/// </param>
/// <param name="PlanOverage">
/// True when the project is on a paid plan and this event is past the plan's included monthly volume. Paid plans keep
/// recording; this lets you notice. Always false for a replay.
/// </param>
/// <param name="RequestId">The API's Request-Id.</param>
public sealed record UsageResult(LedgerEntry Event, long Remaining, bool Replayed, bool PlanOverage, string? RequestId);

/// <summary>A customer's credits. <c>Remaining = Granted - Consumed</c> and is never negative.</summary>
public sealed record Balance(long Remaining, long Consumed, long Granted)
{
    internal static Balance From(Fields f) => new(f.Long("credits"), f.Long("consumed"), f.Long("granted"));
}

/// <summary>The usage an entitlement check should price in, in the meter's own units (not credits).</summary>
public sealed record Estimate(string Meter, long Quantity);

/// <summary>
/// The answer of <see cref="EntitlementsApi.CheckAsync"/>. Advisory: it reserves nothing; the authoritative limit is the debit
/// made by <see cref="UsageApi.RecordAsync"/>. Unless <c>Allowed</c> is true, do not run the work.
/// </summary>
/// <param name="Reason">"ok", "no_active_plan", "feature_not_in_plan" or "insufficient_credits".</param>
/// <param name="Plan">Null when the customer has none.</param>
public sealed record Entitlement(bool Allowed, string Reason, long Remaining, long Required, string? Plan)
{
    internal static Entitlement From(Fields f) =>
        new(f.Bool("allowed"), f.Str("reason"), f.Long("remaining"), f.Long("required"), f.StrOrNull("plan"));
}

/// <summary>
/// A plan of the project. When it comes from a <see cref="Subscription"/> only <c>Key</c> is guaranteed; the other fields are
/// null, 0 or empty if the API did not send them.
/// </summary>
public sealed record Plan(
    string Key,
    string? Name,
    long IncludedCredits,
    string? Meter,
    IReadOnlyList<string> Features,
    DateTimeOffset? UpdatedAt)
{
    internal static Plan From(Fields f) =>
        new(
            f.Str("key"),
            f.StrOrNull("name"),
            f.LongOr("includedCredits", 0),
            f.StrOrNull("meter"),
            f.Has("features") ? f.Strings("features") : Array.Empty<string>(),
            f.TimeOrNull("updatedAt"));

    /// <summary>The full shape <see cref="PlansApi.UpsertPlanAsync"/> promises.</summary>
    internal static Plan Strict(Fields f)
    {
        _ = f.Str("name");
        _ = f.Long("includedCredits");
        _ = f.Str("meter");
        _ = f.Strings("features");
        return From(f);
    }
}

/// <summary>The fields of <see cref="PlansApi.UpsertPlanAsync"/>.</summary>
public sealed record PlanSpec(string Key, string Name, long IncludedCredits, string Meter, IReadOnlyList<string>? Features = null);

/// <summary>A customer's plan.</summary>
/// <param name="Status">"active" or "canceled".</param>
public sealed record Subscription(
    string Id,
    string CustomerId,
    Plan Plan,
    string Status,
    DateTimeOffset? PeriodStart,
    DateTimeOffset? PeriodEnd,
    DateTimeOffset? CreatedAt)
{
    internal static Subscription From(Fields f) =>
        new(
            f.Str("id"),
            f.Str("customerId"),
            Plan.From(f.Obj("plan")),
            f.Str("status"),
            f.TimeOrNull("periodStart"),
            f.TimeOrNull("periodEnd"),
            f.TimeOrNull("createdAt"));
}

/// <summary>
/// A short-lived read token for ONE customer, for the usage widget in a browser. It is shown once; a browser may hold it,
/// never an API key. <see cref="ToString"/> does not print the token.
/// </summary>
public sealed record CustomerToken(string Id, string Token, string CustomerId, DateTimeOffset ExpiresAt)
{
    internal static CustomerToken From(Fields f) => new(f.Str("id"), f.Str("token"), f.Str("customerId"), f.Time("expiresAt"));

    /// <inheritdoc />
    public override string ToString() => $"CustomerToken {{ Id = {Id}, CustomerId = {CustomerId}, ExpiresAt = {ExpiresAt:O}, Token = <hidden> }}";
}

/// <summary>
/// The project's plan and this month's recorded usage. <see cref="UnconfirmedLimits"/> is a working hypothesis, not approved
/// and not a guarantee: <c>Confirmed</c> is false. Do not show it to your customers as a quota, a price or a commitment.
/// </summary>
public sealed record PlanUsage(PlanRef Plan, DateTimeOffset PeriodStart, DateTimeOffset PeriodEnd, long Events, PlanLimits UnconfirmedLimits)
{
    internal static PlanUsage From(Fields f)
    {
        var plan = f.Obj("plan");
        var period = f.Obj("period");
        var limits = f.Obj("unconfirmedLimits");
        return new PlanUsage(
            new PlanRef(plan.Str("key"), plan.StrOrNull("name")),
            period.Time("start"),
            period.Time("end"),
            f.Obj("usage").Long("events"),
            new PlanLimits(
                limits.Bool("confirmed"),
                limits.StrOrNull("note"),
                limits.LongOrNull("includedMonthlyEvents"),
                limits.LongOrNull("activityRetentionDays")));
    }
}

/// <summary>Names a plan.</summary>
public sealed record PlanRef(string Key, string? Name);

/// <summary>Proposed plan limits. Never a guarantee.</summary>
/// <param name="IncludedMonthlyEvents">Null when none is proposed.</param>
/// <param name="ActivityRetentionDays">Null when none is proposed.</param>
public sealed record PlanLimits(bool Confirmed, string? Note, long? IncludedMonthlyEvents, long? ActivityRetentionDays);

/// <summary>One page of a list.</summary>
/// <param name="NextCursor">Opaque: pass it back as <see cref="ListOptions.Cursor"/>. Null on the last page.</param>
public sealed record Page<T>(IReadOnlyList<T> Data, string? NextCursor)
{
    /// <summary>True when another page follows.</summary>
    public bool HasMore => !string.IsNullOrEmpty(NextCursor);
}

/// <summary>Pages a list. <c>Limit</c> is 1 to 100 (0 means the API's default, 25).</summary>
public sealed record ListOptions(int Limit = 0, string? Cursor = null);
