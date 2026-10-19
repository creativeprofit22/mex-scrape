# matching

## Purpose

Links a retailer offer (`domain::Offer`) to an Amazon.com.mx product (`domain::Asin`). Pure logic: no I/O, no clock (the caller passes `matched_at`), integer arithmetic only, deterministic output. The caller (the worker's `offer_matching`) does the Amazon lookups and stores the result. ROI lives in `domain::roi`.

## Public API

- `match_offer(offer, gtin_hits: &[Candidate], keyword_hits: &[Candidate], &MatchConfig, matched_at) -> Option<domain::ProductMatch>`.
- `Candidate { asin, title, brand: Option<String> }`: one Amazon catalog item.
- `MatchConfig::new(accept_bp, floor_bp) -> Result<_, MatchConfigError>` (requires `floor_bp ≤ accept_bp ≤ 10000`), `MatchConfig::default()`, `accept_bp()`, `floor_bp()`.
- `AMBIGUITY_MARGIN_BP = 500`.
- `tokens(text: &str) -> BTreeSet<String>`, `digit_tokens(tokens: &BTreeSet<String>) -> BTreeSet<&str>`, `dice_bp(a: &BTreeSet<String>, b: &BTreeSet<String>) -> u32` (re-exported at the crate root from the private `normalize` module).

## Match rules

Confidence is an integer in basis points, `0..=10000` (10000 = certain).

1. **GTIN first.** When the offer has a GTIN and Amazon returned catalog items for it (`gtin_hits` non-empty), the match uses method `gtin` and confidence 10000. Status is `accepted` only when all hits share one ASIN and no hit's brand conflicts with the offer's brand (both known and normalizing differently). Several distinct ASINs or a brand conflict give `needs_review`, choosing the ASIN with the best title score.
2. **Otherwise title + brand** over `keyword_hits` (method `title`). Per candidate:
   - `title_bp = 2·|A∩B|·10000 / (|A|+|B|)` (token Dice over the normalized title tokens, integer division).
   - Brand factor: ×1 when both brands are known and normalize equal, or when one brand is missing but the other's tokens all appear in that side's title; ×0.9 when there is no brand evidence; ×0 when both are known and differ.
   - Model/size guard: when both titles have tokens containing digits (models, sizes like `500ml`, `1l`) and those sets are disjoint, the score is halved.
   - A candidate repeated in the list keeps its best score.
3. Candidates are ranked by (confidence desc, ASIN asc), so ties always pick the smallest ASIN.
4. Status from the thresholds:
   - top `< floor`: `None` (nothing stored; no plausible candidate).
   - top `≥ accept` and the runner-up is not within `AMBIGUITY_MARGIN_BP` (500 bp): `accepted`.
   - everything else (`floor ≤ top < accept`, or an ambiguous top two even above accept): `needs_review`. Low-confidence matches are never auto-accepted.

## Thresholds

| Setting | Default | Meaning |
|---|---|---|
| `accept_bp` | 8500 (85%) | lowest title confidence that is auto-accepted |
| `floor_bp` | 3000 (30%) | lowest confidence that is stored (as `needs_review`) |
| `AMBIGUITY_MARGIN_BP` | 500 (5%) | runner-up this close to the top forces `needs_review` |

The defaults are a starting point; tune them against real matches.

## Normalization

`tokens(text)`: lowercase; fold Spanish accents with a fixed table (á à ä â → a, é → e, í → i, ó → o, ú ü → u, ñ → n, ç → c); every character that is not an ASCII letter or digit becomes a space; drop the stopwords `a al and con de del el en for la las los of para por sin the un una with y`; collect into a sorted, de-duplicated set. Brands go through the same function.

## Statuses

`domain::MatchStatus`: `accepted`, `needs_review`, `rejected`. The matcher produces only the first two. `rejected` is set by a reviewer (the later AI-assistant or manual review); `db` never overwrites a `rejected` match on re-matching.

## Dependencies

- Workspace: `domain`.

## Owned tables

None. Matches are stored in `offer_matches` (see [db.md](db.md)).
