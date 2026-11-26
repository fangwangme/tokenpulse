package format

import "testing"

// usage-report.md "Numbers": the compact money format every surface uses.
func TestCost(t *testing.T) {
	cases := []struct {
		value float64
		want  string
	}{
		{999.99, "$999.99"},
		{1000, "$1.000K"},
		{9876.54, "$9.877K"},
		{10000, "$10.000K"},
		{1234567, "$1.235M"},
	}
	for _, test := range cases {
		if got := Cost(test.value); got != test.want {
			t.Errorf("Cost(%v) = %q, want %q", test.value, got, test.want)
		}
	}
}
