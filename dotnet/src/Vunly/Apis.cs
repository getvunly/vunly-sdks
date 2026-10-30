using System;
using System.Collections.Generic;
using System.Linq;
using System.Net.Http;
using System.Runtime.CompilerServices;
using System.Threading;
using System.Threading.Tasks;

namespace Vunly;

/// <summary>Creates and reads the customers you bill.</summary>
public sealed class CustomersApi
{
    private readonly Transport _t;

    internal CustomersApi(Transport t) => _t = t;

    /// <summary>
    /// Creates the customer or returns the existing one for this id (your own id). Asking twice leaves one customer.
    /// </summary>
    /// <param name="name">A display name shown in the console; null leaves it unset.</param>
    public async Task<Customer> UpsertAsync(string id, string? name = null, CancellationToken cancellationToken = default)
    {
        var body = new Dictionary<string, object?> { ["externalId"] = id };
        if (!string.IsNullOrEmpty(name))
        {
            body["name"] = name;
        }
        return (await _t.SendAsync(new Transport.Request(HttpMethod.Post, "/v1/customers", null, body, null, Transport.OpClass.N), Customer.From, cancellationToken).ConfigureAwait(false)).Value;
    }

    /// <summary>
    /// One customer, by your id or Vunly's. A customer that does not exist is an <see cref="InvalidRequestException"/> with code
    /// "not_found".
    /// </summary>
    public async Task<Customer> GetAsync(string customer, CancellationToken cancellationToken = default) =>
        (await _t.SendAsync(Transport.Get("/v1/customers/" + Transport.Segment(customer, "customer"), null), Customer.From, cancellationToken).ConfigureAwait(false)).Value;

    /// <summary>One page of customers, newest first.</summary>
    public async Task<Page<Customer>> ListAsync(ListOptions? options = null, CancellationToken cancellationToken = default) =>
        (await _t.SendAsync(Transport.Get("/v1/customers", Transport.Query(options)), f => Pages.Of(f, Customer.From), cancellationToken).ConfigureAwait(false)).Value;

    /// <summary>Every customer, fetched one page at a time and only as you consume them.</summary>
    public IAsyncEnumerable<Customer> ListAllAsync(int pageSize = 0, CancellationToken cancellationToken = default) =>
        Pages.All((cursor, ct) => ListAsync(new ListOptions(pageSize, cursor), ct), cancellationToken);
}

/// <summary>Grants credits and reads balances and the ledger.</summary>
public sealed class CreditsApi
{
    private readonly Transport _t;

    internal CreditsApi(Transport t) => _t = t;

    /// <summary>
    /// Adds credits (a sandbox top-up). <paramref name="id"/> is the Idempotency-Key (8 to 128 characters of
    /// <c>A-Za-z0-9_.:-</c>): repeating the same id grants once and returns the original entry.
    /// </summary>
    public async Task<LedgerEntry> GrantAsync(string id, string customer, long credits, string? reason = null, CancellationToken cancellationToken = default)
    {
        var body = new Dictionary<string, object?> { ["credits"] = credits };
        if (!string.IsNullOrEmpty(reason))
        {
            body["reason"] = reason;
        }
        var request = new Transport.Request(HttpMethod.Post, "/v1/customers/" + Transport.Segment(customer, "customer") + "/credit-grants", null, body, id, Transport.OpClass.K);
        return (await _t.SendAsync(request, LedgerEntry.From, cancellationToken).ConfigureAwait(false)).Value;
    }

    /// <summary>A customer's credits.</summary>
    public async Task<Balance> GetBalanceAsync(string customer, CancellationToken cancellationToken = default) =>
        (await _t.SendAsync(Transport.Get("/v1/customers/" + Transport.Segment(customer, "customer") + "/balance", null), Balance.From, cancellationToken).ConfigureAwait(false)).Value;

    /// <summary>One page of a customer's ledger entries, newest first.</summary>
    public async Task<Page<LedgerEntry>> GetLedgerAsync(string customer, ListOptions? options = null, CancellationToken cancellationToken = default) =>
        (await _t.SendAsync(Transport.Get("/v1/customers/" + Transport.Segment(customer, "customer") + "/ledger", Transport.Query(options)), f => Pages.Of(f, LedgerEntry.From), cancellationToken).ConfigureAwait(false)).Value;

    /// <summary>A customer's whole ledger, one page at a time, only as you consume it.</summary>
    public IAsyncEnumerable<LedgerEntry> GetLedgerAllAsync(string customer, int pageSize = 0, CancellationToken cancellationToken = default) =>
        Pages.All((cursor, ct) => GetLedgerAsync(customer, new ListOptions(pageSize, cursor), ct), cancellationToken);
}

/// <summary>Records usage events.</summary>
public sealed class UsageApi
{
    private readonly Transport _t;

    internal UsageApi(Transport t) => _t = t;

    /// <summary>
    /// Records usage once and debits credits. It is final: a <see cref="LimitReachedException"/> means the credits were not there
    /// when you recorded, even if a check said yes earlier; nothing was debited. A repeat of the same id returns the original
    /// result with <c>Replayed == true</c>.
    /// </summary>
    /// <param name="id">
    /// Names the unit of work (your request or job id) and is sent as the Idempotency-Key: 8 to 128 characters of
    /// <c>A-Za-z0-9_.:-</c>. Retry with the SAME id; the API applies it at most once. It is never generated for you.
    /// </param>
    /// <param name="quantity">In the meter's own units (for example tokens), not credits: the project's meter converts it, rounded up per request.</param>
    public async Task<UsageResult> RecordAsync(string id, string customer, string meter, long quantity, CancellationToken cancellationToken = default)
    {
        var body = new Dictionary<string, object?> { ["customerId"] = customer, ["meter"] = meter, ["quantity"] = quantity };
        var request = new Transport.Request(HttpMethod.Post, "/v1/usage-events", null, body, id, Transport.OpClass.K);
        var (value, meta) = await _t.SendAsync(request, f => (Event: LedgerEntry.From(f.Obj("event")), Balance: f.Long("balance")), cancellationToken).ConfigureAwait(false);
        return new UsageResult(value.Event, value.Balance, meta.Replayed, meta.PlanOverage && !meta.Replayed, meta.RequestId);
    }
}

/// <summary>Asks whether a customer may do something now.</summary>
public sealed class EntitlementsApi
{
    private readonly Transport _t;

    internal EntitlementsApi(Transport t) => _t = t;

    /// <summary>
    /// Read-only and advisory. If it throws (an <see cref="UnavailableException"/> after retries), treat that as "no": fail closed,
    /// do not run the work. It never returns <c>Allowed == true</c> by itself.
    /// </summary>
    /// <param name="estimate">Optional: the usage the work is expected to need.</param>
    public async Task<Entitlement> CheckAsync(string customer, string feature, Estimate? estimate = null, CancellationToken cancellationToken = default)
    {
        var body = new Dictionary<string, object?> { ["customerId"] = customer, ["feature"] = feature };
        if (estimate is not null)
        {
            body["estimate"] = new Dictionary<string, object?> { ["meter"] = estimate.Meter, ["quantity"] = estimate.Quantity };
        }
        var request = new Transport.Request(HttpMethod.Post, "/v1/entitlements/check", null, body, null, Transport.OpClass.R);
        return (await _t.SendAsync(request, Entitlement.From, cancellationToken).ConfigureAwait(false)).Value;
    }
}

/// <summary>Meters, plans and subscriptions.</summary>
public sealed class PlansApi
{
    private readonly Transport _t;

    internal PlansApi(Transport t) => _t = t;

    /// <summary>Creates or updates a meter: how many units of usage cost one credit.</summary>
    public async Task UpsertMeterAsync(string key, long unitsPerCredit, CancellationToken cancellationToken = default)
    {
        var body = new Dictionary<string, object?> { ["key"] = key, ["unitsPerCredit"] = unitsPerCredit };
        await _t.SendAsync(new Transport.Request(HttpMethod.Post, "/v1/meters", null, body, null, Transport.OpClass.N), cancellationToken).ConfigureAwait(false);
    }

    /// <summary>Creates or updates a plan.</summary>
    public async Task<Plan> UpsertPlanAsync(PlanSpec spec, CancellationToken cancellationToken = default)
    {
        ArgumentNullException.ThrowIfNull(spec);
        var body = new Dictionary<string, object?>
        {
            ["key"] = spec.Key,
            ["name"] = spec.Name,
            ["includedCredits"] = spec.IncludedCredits,
            ["meter"] = spec.Meter,
            ["features"] = (spec.Features ?? Array.Empty<string>()).ToList(),
        };
        return (await _t.SendAsync(new Transport.Request(HttpMethod.Post, "/v1/plans", null, body, null, Transport.OpClass.N), Plan.Strict, cancellationToken).ConfigureAwait(false)).Value;
    }

    /// <summary>
    /// Activates a plan for the customer. The same plan again returns the existing subscription and grants nothing more; another
    /// plan while one is active is an <see cref="InvalidRequestException"/> with code "subscription_exists".
    /// </summary>
    public async Task<Subscription> SubscribeAsync(string customer, string plan, CancellationToken cancellationToken = default)
    {
        var request = new Transport.Request(
            HttpMethod.Post,
            "/v1/customers/" + Transport.Segment(customer, "customer") + "/subscription",
            null,
            new Dictionary<string, object?> { ["plan"] = plan },
            null,
            Transport.OpClass.N);
        return (await _t.SendAsync(request, Subscription.From, cancellationToken).ConfigureAwait(false)).Value;
    }

    /// <summary>Cancels the customer's active subscription.</summary>
    public async Task CancelAsync(string customer, CancellationToken cancellationToken = default) =>
        await _t.SendAsync(new Transport.Request(HttpMethod.Delete, "/v1/customers/" + Transport.Segment(customer, "customer") + "/subscription", null, null, null, Transport.OpClass.N), cancellationToken).ConfigureAwait(false);
}

/// <summary>Short-lived read tokens for one customer, for a browser.</summary>
public sealed class CustomerTokensApi
{
    private readonly Transport _t;

    internal CustomerTokensApi(Transport t) => _t = t;

    /// <summary>
    /// Mints a token for ONE customer. <paramref name="ttl"/> is 30 seconds to 1 hour (null means 15 minutes). Every call mints a
    /// new token and has no idempotency key, so after a lost answer the SDK does NOT repeat it: it throws an
    /// <see cref="UnavailableException"/> and you decide.
    /// </summary>
    public async Task<CustomerToken> CreateAsync(string customer, TimeSpan? ttl = null, CancellationToken cancellationToken = default)
    {
        var body = new Dictionary<string, object?> { ["customerId"] = customer };
        if (ttl is { } t)
        {
            body["ttlSeconds"] = (long)t.TotalSeconds;
        }
        var request = new Transport.Request(HttpMethod.Post, "/v1/customer-tokens", null, body, null, Transport.OpClass.X);
        return (await _t.SendAsync(request, CustomerToken.From, cancellationToken).ConfigureAwait(false)).Value;
    }

    /// <summary>Revokes a token by its id. Idempotent.</summary>
    public async Task RevokeAsync(string id, CancellationToken cancellationToken = default) =>
        await _t.SendAsync(new Transport.Request(HttpMethod.Delete, "/v1/customer-tokens/" + Transport.Segment(id, "id"), null, null, null, Transport.OpClass.N), cancellationToken).ConfigureAwait(false);
}

internal static class Pages
{
    internal static Page<T> Of<T>(Fields f, Func<Fields, T> item) =>
        new(f.Objects("data").Select(item).ToList(), f.StrOrNull("nextCursor"));

    /// <summary>Turns a page fetcher into a lazy async sequence: the next page is fetched only when the previous one is consumed.</summary>
    internal static async IAsyncEnumerable<T> All<T>(
        Func<string?, CancellationToken, Task<Page<T>>> fetch,
        [EnumeratorCancellation] CancellationToken cancellationToken = default)
    {
        string? cursor = null;
        // A server that answers the same cursor again (or an empty page that still points onward) would keep this loop
        // fetching forever, as fast as the network allows. A cursor must be new and a page that points onward must have items.
        var seen = new HashSet<string>(StringComparer.Ordinal);
        var emptyInARow = 0;
        while (true)
        {
            var page = await fetch(cursor, cancellationToken).ConfigureAwait(false);
            foreach (var item in page.Data)
            {
                yield return item;
            }
            if (!page.HasMore)
            {
                yield break;
            }
            // an empty page that still promises another is tolerated twice in a row, never a third time (each has a fresh cursor)
            emptyInARow = page.Data.Count == 0 ? emptyInARow + 1 : 0;
            if (emptyInARow >= 3)
            {
                throw VunlyException.InvalidResponse(200, null, "the third consecutive empty page promised another page; stopped");
            }
            if (!seen.Add(page.NextCursor!))
            {
                throw VunlyException.InvalidResponse(200, null, "the next-page cursor did not advance; stopped");
            }
            cursor = page.NextCursor;
        }
    }
}
