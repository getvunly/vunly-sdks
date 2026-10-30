package com.vunly;

/** The usage an entitlement check should price in, in the meter's own units (not credits). */
public record Estimate(String meter, long quantity) {}
