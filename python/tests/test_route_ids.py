"""An empty, "." or ".." id in a path would change the route on the wire (``GET /v1/customers/``, ``GET /v1/``): it is
refused before any request, with ``InvalidRequest`` and ``fields.id``."""

from __future__ import annotations

import httpx
import pytest

from vunly import Client, InvalidRequest

KEY = "vk_test_synthetic_python_route_ids_0001"


@pytest.mark.parametrize("bad", ["", ".", ".."])
def test_an_id_that_would_change_the_route_is_refused_before_any_request(bad: str) -> None:
    hits: list[str] = []

    def handler(request: httpx.Request) -> httpx.Response:
        hits.append(str(request.url))
        return httpx.Response(200, json={})

    c = Client(KEY, "http://api.invalid", max_retries=0, transport=httpx.MockTransport(handler))
    for call in (lambda: c.customers.get(bad), lambda: c.credits.balance(bad)):
        with pytest.raises(InvalidRequest) as info:
            call()
        assert info.value.details == {"fields": {"id": 'not empty, "." or ".."'}}
    assert hits == []
