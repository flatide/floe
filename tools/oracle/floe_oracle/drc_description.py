"""Conservative measurement conditions from DRC rule descriptions.

Descriptions are prose, not executable rules. Only a single, unambiguous
dimension and explicit unit are accepted. Requirement wording is inverted
to an error predicate; a bare comparator describes the error itself.
The original evidence remains separate from the normalized selector text.
"""

import math
import re
from decimal import Decimal, DecimalException


_NUMBER = r"[+-]?(?:\d+(?:\.\d*)?|\.\d+)(?:[eE][+-]?\d+)?"
_QUANTITY = re.compile(
    r"(?<![\w.])(?P<value>" + _NUMBER + r")\s*"
    r"(?P<unit>um|nm|micromet(?:er|re)s?|microns?|nanomet(?:er|re)s?|"
    r"마이크로미터|나노미터)(?P<power>\^?2)?(?![\w^])", re.I)
_COMPARATOR = re.compile(r"(<=|>=|==|!=|<|>|=)\s*$")
_INVERSE = {"<": ">=", "<=": ">", ">": "<=", ">=": "<"}
_METRICS = {
    "width": "width", "폭": "width", "선폭": "width",
    "space": "space", "spacing": "space", "간격": "space", "이격": "space",
    "notch": "notch", "노치": "notch",
    "enclosure": "enclosure", "enc": "enclosure", "인클로저": "enclosure",
    "overlap": "overlap", "겹침": "overlap", "중첩": "overlap",
    "extension": "extension", "연장": "extension", "확장": "extension",
    "length": "length", "길이": "length", "area": "area", "면적": "area",
}
_RULE_METRIC = re.compile(r"[A-Za-z]+|[가-힣]+")
_MINIMUM = re.compile(r"\b(?:minimum|min)\b|최소", re.I)
_MAXIMUM = re.compile(r"\b(?:maximum|max)\b|최대", re.I)
_REQUIRED = re.compile(r"\b(?:must|shall|should)\s+be\b", re.I)
_AMBIGUOUS = re.compile(
    r"\b(?:if|unless|except|when|otherwise|or|and|between|range|depending|"
    r"only|inside|outside|within|depends|dependent|condition|conditional|provided|applies)\b|"
    r"경우|조건|영역|또는|혹은|제외|따라|일\s*때", re.I)
_NOTE = re.compile(
    r"^(?:Rule\s+File\s+(?:Pathname|Title)\s*:|"
    r"(?:selected\s+|actual\s+|measured\s+)?CD\b"
    r"(?!\s*(?:[<>=]|(?:must|shall|should)\s+be))|"
    r"expected\b|examples?\b|local\s+#|#\d|waiv(?:er|ed|able)\b|"
    r"측정값|예시|예제|예상\s*(?:값|델타))", re.I)
_SIGNAL = re.compile(
    r"[<>=]|\b(?:minimum|maximum|min|max|limit)\b|최소|최대|이상|이하|미만|초과", re.I)
_KOREAN_COMPARISON = re.compile(
    r"^(이상|이하|초과|미만)\s*(?:(?:이?어야|여야)\s*(?:한다|함|합니다|됨|됩니다)?)?$"
)


def _normalized(text):
    text = re.sub(r"\b(min|max)\.(?=\s)", r"\1", text, flags=re.I)
    return " ".join(text.replace("µ", "u").replace("μ", "u")
                    .replace("²", "^2").replace("≥", ">=")
                    .replace("≤", "<=").split())


def _name_metric(name):
    found = {_METRICS[word.lower()] for word in _RULE_METRIC.findall(str(name))
             if word.lower() in _METRICS}
    return next(iter(found)) if len(found) == 1 else None


def _metric(prefix, name):
    # These two phrases distinguish the samples' otherwise ambiguous SVRF
    # operator names from the explicit physical dimension in the description.
    prefix = re.sub(r"^two[- ]layer\s+internal\s+overlap$", "overlap", prefix, flags=re.I)
    prefix = re.sub(r"^enclosure\s+extension$", "extension", prefix, flags=re.I)
    prefix = re.sub(r"(?:은|는|이|가|의)$", "", prefix).strip()
    if not prefix or prefix.lower() in ("dimension", "cd", "치수"):
        return _name_metric(name)
    return _METRICS.get(prefix.lower())


def _clause(name, evidence):
    text = _normalized(evidence)
    if _AMBIGUOUS.search(text):
        return None
    quantities = list(_QUANTITY.finditer(text))
    if len(quantities) != 1:
        return None
    quantity = quantities[0]
    prefix = text[:quantity.start()].strip()
    suffix = text[quantity.end():].strip().rstrip(".").strip()
    if len(quantity["value"]) > 64:
        return None
    try:
        original = value = Decimal(quantity["value"])
        squared = bool(quantity["power"])
        nanometers = quantity["unit"].lower().startswith("n") or quantity["unit"] == "나노미터"
        if nanometers:
            value /= Decimal(1000000 if squared else 1000)
        numeric = float(value)
        if original < 0 or not math.isfinite(numeric) or (original and not numeric):
            return None
        # Scientific notation keeps extreme but finite inputs bounded in size.
        bound = str(value.normalize())
    except (DecimalException, OverflowError, ValueError):
        return None

    # A literal rule-name label is harmless; arbitrary layer phrases are not
    # dropped, because they may qualify which distance is constrained.
    for label in (str(name) + ":", str(name) + " :"):
        if prefix.startswith(label):
            prefix = prefix[len(label):].strip()
            break
    requirement = bool(_REQUIRED.search(prefix))
    prefix = _REQUIRED.sub("", prefix).strip()
    # Explicit verbal bounds are supported only with requirement wording.
    if requirement:
        for phrase, operator in (("at least", ">="), ("at most", "<="),
                                 ("greater than", ">"), ("less than", "<")):
            if prefix.lower().endswith(phrase):
                prefix = prefix[:-len(phrase)].rstrip() + " " + operator
                break
    operator = None
    comparator = _COMPARATOR.search(prefix)
    if comparator:
        operator = comparator[1]
        prefix = prefix[:comparator.start()].strip()

    minimum = bool(_MINIMUM.search(prefix) or _MINIMUM.fullmatch(suffix))
    maximum = bool(_MAXIMUM.search(prefix) or _MAXIMUM.fullmatch(suffix))
    if minimum and maximum:
        return None
    prefix = _MINIMUM.sub("", _MAXIMUM.sub("", prefix)).strip()
    prefix = re.sub(r"\s+(?:is|of)$", "", prefix, flags=re.I).strip()
    if _MINIMUM.fullmatch(suffix) or _MAXIMUM.fullmatch(suffix):
        suffix = ""
    if suffix:
        korean = _KOREAN_COMPARISON.fullmatch(suffix)
        if korean is None or operator is not None:
            return None
        operator = {"이상": ">=", "이하": "<=", "초과": ">", "미만": "<"}[korean[1]]
        requirement |= suffix != korean[1]
    metric = _metric(prefix, name)
    if metric is None or squared != (metric == "area"):
        return None
    if minimum or maximum:
        allowed = (">", ">=") if minimum else ("<", "<=")
        if operator in (None, "="):
            operator = "<" if minimum else ">"
        elif operator in allowed:
            operator = _INVERSE[operator]
        else:
            return None
    elif requirement:
        operator = _INVERSE.get(operator)
    if operator not in _INVERSE:
        return None
    return {"metric": metric, "op": operator, "value": numeric,
            "text": "%s %s %s" % (metric, operator, bound),
            "source": "description", "description": evidence.strip()}


def description_constraints(name, description):
    """Return one evidenced error condition, or [] when prose is unresolved.

    Units must be explicit (length um/nm, area um²/nm²). A minimum or
    maximum is a requirement; a bare comparator describes emitted errors.
    Conditional rules, ranges and multiple distinct conditions are withheld.
    This is inference from prose, so every returned record carries provenance.
    """
    if not isinstance(description, str) or len(description) > 65536:
        return []
    conditions = []
    for line in description.splitlines():
        line = line.strip().lstrip("@").strip()
        if not line or _NOTE.match(line):
            continue
        # Periods inside decimal numbers are never sentence boundaries.
        clauses = re.split(
            r";|(?<!min)(?<!max)(?<!\d)\.(?=\s)|(?<=\d)\.(?=\s+[A-Za-z#가-힣])",
            line, flags=re.I)
        for evidence in clauses:
            evidence = evidence.strip()
            if not evidence or _NOTE.match(evidence):
                continue
            text = _normalized(evidence)
            if _AMBIGUOUS.search(text):
                return []
            condition = _clause(name, evidence)
            if condition is None:
                # Ignore ordinary narrative, but never discard an unresolved
                # second condition and incorrectly treat the first as complete.
                if _SIGNAL.search(text) or _QUANTITY.search(text):
                    return []
                continue
            conditions.append(condition)
    if not conditions:
        return []
    keys = {(c["metric"], c["op"], c["value"]) for c in conditions}
    return [conditions[0]] if len(keys) == 1 else []


def constraints_for(check, metadata=None):
    """Prefer explicit SVRF constraints, using description only when absent."""
    checks = (metadata or {}).get("checks") or {}
    constraints = (checks.get(check.name) or {}).get("constraints")
    if constraints:
        return constraints
    return description_constraints(check.name, getattr(check, "desc", ""))
