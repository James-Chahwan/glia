#!/bin/bash
# Apply a model output's diff to marshmallow-1359 + run FAIL_TO_PASS + regression suite.
# Usage: apply_test.sh <out_file> <label>
set -e
OUT_FILE="$1"
LABEL="$2"
REPO=/home/ivy/swe-work/marshmallow-1359

cd "$REPO"
git checkout -- src/marshmallow/fields.py 2>/dev/null || true

# Extract diff block from output (strip markdown fences if any, stop at <|im_end|>)
python3 - "$OUT_FILE" <<'PY' > /tmp/patch.diff
import sys, re
text = open(sys.argv[1]).read()
# strip <|im_end|> and later
text = text.split('<|im_end|>')[0]
# find first 'diff --git' or '--- a/'
m = re.search(r'(diff --git .*|--- a/.*)', text, re.DOTALL)
if not m:
    sys.exit("no diff found")
diff = m.group(0)
# drop trailing markdown fence line
diff = re.sub(r'\n```.*$', '', diff, flags=re.DOTALL)
# some outputs have the fake index line '1234567..89abcdef' — git apply tolerates if we use --recount
print(diff)
PY

# Try to apply the diff
if git apply --recount --whitespace=fix /tmp/patch.diff 2>/tmp/apply.err; then
  APPLY="applied"
else
  # try patch with fuzz
  if patch -p1 --forward --quiet < /tmp/patch.diff 2>/dev/null; then
    APPLY="applied(fuzz)"
  else
    echo "$LABEL: APPLY-FAIL"
    cat /tmp/apply.err
    git checkout -- src/marshmallow/fields.py 2>/dev/null
    exit 0
  fi
fi

# Now run FAIL_TO_PASS test + regression
F2P_RESULT="?"
REG_RESULT="?"

python3 - <<'PY' > /tmp/f2p.log 2>&1 || true
try:
    from marshmallow import fields, Schema

    class MySchema(Schema):
        foo = fields.List(fields.DateTime())
        bar = fields.Tuple((fields.DateTime(),))

        class Meta:
            datetimeformat = 'iso8601'
            dateformat = 'iso8601'

    schema = MySchema()
    a = schema.fields['foo'].inner.format
    b = schema.fields['bar'].tuple_fields[0].format
    if a == 'iso8601' and b == 'iso8601':
        print("F2P=PASS")
    else:
        print(f"F2P=FAIL (foo.format={a!r}, bar[0].format={b!r})")
except Exception as e:
    print(f"F2P=EXC ({type(e).__name__}: {e})")
PY
F2P_RESULT=$(grep -oE 'F2P=[A-Z]+' /tmp/f2p.log | head -1 || echo "F2P=?")
F2P_DETAIL=$(cat /tmp/f2p.log)

# Run regression suite
REG=$(python -m pytest tests/test_fields.py --no-header -q 2>&1 | grep -E "passed|failed|error" | tail -1)

git checkout -- src/marshmallow/fields.py 2>/dev/null

echo "$LABEL | apply=$APPLY | $F2P_RESULT | reg: $REG"
