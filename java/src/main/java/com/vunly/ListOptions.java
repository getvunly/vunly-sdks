package com.vunly;

/**
 * Pages a list. {@code limit} is 1 to 100 (0 means the API's default, 25).
 *
 * @param cursor null or empty for the first page
 */
public record ListOptions(int limit, String cursor) {
  /** The API's default page size, first page. */
  public static ListOptions defaults() {
    return new ListOptions(0, null);
  }

  public static ListOptions limit(int limit) {
    return new ListOptions(limit, null);
  }

  public ListOptions after(String nextCursor) {
    return new ListOptions(limit, nextCursor);
  }
}
