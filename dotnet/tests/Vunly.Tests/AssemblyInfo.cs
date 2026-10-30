using Xunit;

// The memory tests measure allocation and run under a 256 MB heap limit (runtimeconfig.template.json): tests that build bodies of
// 10 to 16 MB must not run next to each other.
[assembly: CollectionBehavior(DisableTestParallelization = true)]
