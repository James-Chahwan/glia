# Channel attribution — cycle 0.3

PASS instances analyzed: 1

## Channel hit rate
| channel | hits | of PASSes | rate |
|---|---|---|---|
| legacy_traceback | 1 | 1 | 100% |

## Per-instance attribution
| instance | gold_qname | primary | all_attributed | legacy? |
|---|---|---|---|---|
| marshmallow-code__marshmallow-1359 | `_bind_to_schema` | legacy_traceback | legacy_traceback | yes |

## Interpretation

- **All PASSes attributed to one channel** → B4 composer is over-engineered for the current corpus; one synth bin carries the wins.
- **PASSes attributed to multiple channels with cross-channel agreement** → B4 composer is the load-bearing piece; redundancy lifts confidence.
- **No channel attributed (gold_qname not in any channel's bullets)** → either gold extraction failed (check gold_qname column) or the PASS came from sub-symbol guidance (line numbers, exception class) rather than named-target channels — diagnostic gap to close.
