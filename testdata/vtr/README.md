# VTR samples

Files for testing `src/fpga`, all from
[verilog-to-routing](https://github.com/verilog-to-routing/vtr-verilog-to-routing) at
commit `4aa4420851326afcf0d81133873e6f48aa44ab76`. They are data, and nothing here is
ever executed.

| File | From | Notes |
| --- | --- | --- |
| `mult_4x4.net`, `.place`, `.route` | `vpr/test/gui/fixtures/clean/` | Unmodified VPR output, for `mult_4x4` on `k6_N10_40nm.xml` |
| `multiclock.net` | `vtr_flow/tasks/regression_tests/vtr_reg_strong/strong_place/` | Unmodified, for global nets |
| `unconnected.net` | `vpr/test/` | Unmodified, for multi-bit ports |
| `apex2_block_locations.place` | `vtr_flow/tasks/regression_tests/vtr_reg_strong/strong_fix_clusters/` | Unmodified, a partial placement |
| `z1000_corner.rr_graph.xml` | `vtr_flow/arch/zeroasic/z1000/z1000_rr_graph.zip` | **An excerpt, not the file as distributed**: the tiles up to (2, 2) of the graph, with nodes renumbered, most metadata removed, and the switches, segments and block types that it uses |

The full z1000 graph is 96 MB, too large to keep here. Tests that use it are `#[ignore]`d
and look for it through the `VTR_RR_GRAPH` environment variable.

## Licence

VTR's `LICENSE.md` puts its files under the MIT licence, copied below, unless noted
otherwise, and notes its benchmark circuits and ABC as exceptions.

- The `.net`, `.place` and `.route` files are VPR's output, kept among VPR's own test
  files, and are treated here as covered by the MIT licence. They describe circuits packed
  into LUTs and flip-flops. `mult_4x4.blif`, the source of `mult_4x4`, states no terms of its
  own. The sources of `multiclock`, `unconnected` and `apex2` were not traced, and `apex2`
  is an MCNC benchmark, so check those if redistributing them matters beyond these tests.
- `z1000_corner.rr_graph.xml` comes from VTR's capture of the Zero ASIC z1000 architecture.
  VTR's `vtr_flow/arch/zeroasic/README.md` traces this to
  [logiklib](https://github.com/siliconcompiler/logiklib), whose repository is under the
  Apache-2.0 licence (<https://www.apache.org/licenses/LICENSE-2.0>). So the file is treated
  as under that licence as well as VTR's, and it is modified as described above.

```
The MIT License (MIT)

Copyright 2012 VTR Developers

Permission is hereby granted, free of charge, to any person obtaining a copy of
this software and associated documentation files (the "Software"), to deal in
the Software without restriction, including without limitation the rights to
use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies
of the Software, and to permit persons to whom the Software is furnished to do
so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```
