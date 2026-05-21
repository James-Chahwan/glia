# Failure mode classification — cycle 0.3

Instances: 7

| Mode | Count | % |
|---|---|---|
| PASS | 1 | 14% |
| RIGHT-LINE-WRONG-CONTENT | 2 | 29% |
| RIGHT-TARGET-WRONG-EDIT | 2 | 29% |
| WRONG-TARGET | 0 | 0% |
| APPLY-FAIL | 1 | 14% |
| NO-DIFF | 1 | 14% |

## Per-instance table
| instance | mode | apply | f2p | model_files | gold_files | model_lines | gold_lines |
|---|---|---|---|---|---|---|---|
| sphinx-doc__sphinx-10325 | APPLY-FAIL | APPLY-FAIL | — | sphinx/ext/autodoc.py | sphinx/ext/autodoc/__init__.py | sphinx/ext/autodoc.py:123-129 | sphinx/ext/autodoc/__init__.py:109-122, sphinx/ext/autodoc/__init__.py:682-692 |
| matplotlib__matplotlib-22835 | NO-DIFF | NO-DIFF | — | — | lib/matplotlib/artist.py | — | lib/matplotlib/artist.py:12-18, lib/matplotlib/artist.py:1304-1323 |
| marshmallow-code__marshmallow-1359 | PASS | applied(fuzz) | PASS | src/marshmallow/fields.py | src/marshmallow/fields.py | src/marshmallow/fields.py:1113-1119 | src/marshmallow/fields.py:1114-1120 |
| django__django-10914 | RIGHT-LINE-WRONG-CONTENT | applied(fuzz) | FAIL (0/1 pass, 1 fail, 0 missing) | django/conf/global_settings.py | django/conf/global_settings.py | django/conf/global_settings.py:304-310 | django/conf/global_settings.py:304-310 |
| matplotlib__matplotlib-22711 | RIGHT-LINE-WRONG-CONTENT | applied | FAIL (0/2 pass, 2 fail, 0 missing) | lib/matplotlib/widgets.py | lib/matplotlib/widgets.py | lib/matplotlib/widgets.py:915-921 | lib/matplotlib/widgets.py:813-822, lib/matplotlib/widgets.py:839-854, lib/matplotlib/widgets.py:912-933 |
| pytest-dev__pytest-11143 | RIGHT-TARGET-WRONG-EDIT | applied(fuzz) | FAIL (0/1 pass, 1 fail, 0 missing) | src/_pytest/assertion/rewrite.py | src/_pytest/assertion/rewrite.py | src/_pytest/assertion/rewrite.py:744-750 | src/_pytest/assertion/rewrite.py:676-682 |
| scikit-learn__scikit-learn-10508 | RIGHT-TARGET-WRONG-EDIT | applied(fuzz) | NO-RUN (============================== 2 errors in 0.11s ===============================) | sklearn/preprocessing/label.py | sklearn/preprocessing/label.py | sklearn/preprocessing/label.py:115-121 | sklearn/preprocessing/label.py:126-134, sklearn/preprocessing/label.py:150-159 |

## Reading the histogram

- **PASS** is the win condition.
- **NO-DIFF** means the model emitted no parseable diff — most likely a directive contradiction or context overflow. Diagnose via `out.txt` content.
- **APPLY-FAIL** means the diff is malformed or fuzz tolerance exceeded — typically a hunk-header line-number drift. See `feedback_swebench_apply_check_fuzz_first`.
- **WRONG-TARGET** means the model picked a different file. Indicates the directive's named target was unconvincing or absent.
- **RIGHT-TARGET-WRONG-EDIT** means the right file, wrong line region. Often a context-window distraction.
- **RIGHT-LINE-WRONG-CONTENT** is the closest miss — right place, wrong fix. The compositional gap cycle 0.4 lens identified.
