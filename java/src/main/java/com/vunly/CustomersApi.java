package com.vunly;

import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

/** Creates and reads the customers you bill. */
public final class CustomersApi {
  private final Transport t;

  CustomersApi(Transport t) {
    this.t = t;
  }

  /** Creates the customer or returns the existing one for this id (your own id). Asking twice leaves one customer. */
  public Customer upsert(String id) {
    return upsert(id, null);
  }

  /** As {@link #upsert(String)}, with a display name shown in the console (null leaves it unset). */
  public Customer upsert(String id, String name) {
    Map<String, Object> body = new LinkedHashMap<>();
    body.put("externalId", id);
    if (name != null && !name.isEmpty()) {
      body.put("name", name);
    }
    return t.call(
            new Transport.Request("POST", "/v1/customers", null, body, null, Transport.OpClass.N),
            Customer::from)
        .value();
  }

  /**
   * One customer, by your id or Vunly's. A customer that does not exist is an {@link InvalidRequestException} with code
   * "not_found".
   */
  public Customer get(String customer) {
    return t.call(
            Transport.Request.get("/v1/customers/" + Transport.segment(customer), null), Customer::from)
        .value();
  }

  /** One page of customers, newest first. */
  public Page<Customer> list(ListOptions options) {
    return t.call(Transport.Request.get("/v1/customers", Transport.query(options)), f -> page(f, Customer::from))
        .value();
  }

  /** Every customer, fetched one page at a time and only as you consume them. */
  public Iterable<Customer> listAll(int pageSize) {
    return Pages.iterable(cursor -> list(new ListOptions(pageSize, cursor)));
  }

  static <T> Page<T> page(Fields f, java.util.function.Function<Fields, T> item) {
    List<Fields> raw = f.objects("data");
    return new Page<>(raw.stream().map(item).toList(), f.strOrNull("nextCursor"));
  }
}
