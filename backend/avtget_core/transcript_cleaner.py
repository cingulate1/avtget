"""
Transcript cleaning via Ollama (gemma4:31b-cloud through the local Ollama API).

Claude cleaning is handled by the Rust backend as a two-turn `claude -p --resume`
invocation of the clean-transcript skill — see rust/crates/backend/src/postprocess.rs.
This module never sees a "claude" cleaner selector.

Sharding:
- All token counting uses tiktoken o200k_base (no more Anthropic API dependency)
- Shard at 2500 tokens
- Each shard processed sequentially, results concatenated
"""

import json
import math
import re
import threading
import warnings
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Callable, Dict, List, Optional, Tuple
import socket
from urllib.error import HTTPError, URLError
from urllib.request import Request, urlopen

try:
    import tiktoken
    TIKTOKEN_AVAILABLE = True
except ImportError:
    TIKTOKEN_AVAILABLE = False


def count_tokens(text: str) -> int:
    """Count tokens using tiktoken o200k_base encoding."""
    token_count = _count_tokens_tiktoken(text)
    if token_count is not None:
        return token_count
    return len((text or "").split())


def calculate_num_ctx(transcript_tokens: int, prompt_tokens: int) -> int:
    """Calculate optimal num_ctx as next power of 2.

    Formula: 2^ceil(log2(((6 * transcript_tokens) + prompt_tokens) * 1.05))
    - 6x transcript_tokens: input + output copy + generous margin for thinking/planning
    - prompt_tokens: the full prompt including the transcript
    - 1.05: 5% headroom
    """
    raw = ((6 * transcript_tokens) + prompt_tokens) * 1.05
    return 2 ** math.ceil(math.log2(raw))


OLLAMA_HOST = "http://localhost:11434"
OLLAMA_MODEL = "gemma4:31b-cloud"
OLLAMA_SYSTEM_PROMPT = "<|think|>\nYou are a transcript cleaning engine. You receive raw auto-caption transcripts and output cleaned versions. You never narrate your process, never acknowledge instructions, and never produce commentary. Your entire output IS the cleaned transcript—nothing more."

# ---------------------------------------------------------------------------
# Reference material for domain-specific transcript cleaning (Ollama)
# Source: clean-transcript skill v18 reference files
# ---------------------------------------------------------------------------
_REFERENCES_DIR = Path(__file__).parent / "references"

# Domain detection: keyword sets mapped to reference file names.
# A domain matches when EITHER:
#   - >= _DOMAIN_MATCH_THRESHOLD distinct keywords appear, OR
#   - any single keyword appears >= _DOMAIN_INSTANCE_THRESHOLD times.
# Keywords are case-insensitive and chosen to be distinctive enough to avoid
# false positives from general English.
DOMAIN_KEYWORDS: Dict[str, List[str]] = {
    "bleach_terminology.md": [
        "Zanpakuto", "Bankai", "Soul Society", "Seireitei", "Gotei",
        "Quincy", "Arrancar", "Hueco Mundo", "Hogyoku", "Aizen",
        "Yhwach", "Sternritter", "Ichigo", "Urahara", "reiatsu",
        "Shikai", "Espada", "Hollowfication", "Fullbring",
    ],
    "naruto_boruto_terminology.md": [
        "Sharingan", "Hokage", "chakra", "jutsu", "Rinnegan",
        "Byakugan", "Akatsuki", "jinchuriki", "Susanoo", "Mangekyo",
        "Otsutsuki", "Boruto", "Kurama", "Baryon Mode", "Kage",
        "Amaterasu", "Tsukuyomi", "Rasengan", "Senjutsu",
    ],
    "dnd_terminology.md": [
        "Critical Role", "Dungeon Master", "d20", "Baldur's Gate",
        "Mind Flayer", "Vecna", "Tiamat", "Forgotten Realms",
        "saving throw", "homebrew", "multiclass", "Lolth",
        "paladin", "warlock", "TTRPG", "TPK", "Owlbear",
    ],
    "rocket_league_terminology.md": [
        # Game & gameplay mechanics
        "Rocket League", "RLCS", "flip reset", "ceiling shot",
        "air dribble", "fast aerial", "half flip", "speedflip",
        "speed flip", "wavedash", "wave dash", "shadow defense",
        "Grand Champ", "Supersonic Legend", "musty flick",
        "kickoff", "supersonic",
        # Esports orgs
        "Psyonix", "Vitality", "Karmine Corp", "GenG", "Gen.G",
        "NRG", "BeastMode", "Shopify Rebellion", "Team BDS",
        "Dignitas", "Moist Esports", "Spacestation", "Team Falcons",
        "Oxygen Esports",
        # Players & creators
        "Firstkiller", "Musty", "GarrettG", "SquishyMuffinz",
        "Turbopolsa", "Kaydop", "Fairy Peak", "Jstn", "Jknaps",
        "Retals", "Sypical", "Joyo",
    ],
    "ai_terminology.md": [
        "LLM", "GPT", "transformer", "RLHF", "fine-tuning",
        "Anthropic", "DeepSeek", "Mistral", "Claude Code",
        "RAG", "embeddings", "tokenizer", "SWE-bench",
        "Cursor", "backpropagation", "distillation", "LoRa",
    ],
    "owen_cook_real_social_dynamics_ecosystem.md": [
        "Owen Cook", "Julien Blanc", "RSD",
    ],
}

_DOMAIN_MATCH_THRESHOLD = 2
_DOMAIN_INSTANCE_THRESHOLD = 4
_DOMAIN_MATCH_THRESHOLD_OVERRIDES: Dict[str, int] = {
    "owen_cook_real_social_dynamics_ecosystem.md": 1,
}


def _detect_domains(transcript: str) -> List[str]:
    """Scan transcript for domain-specific keywords and return matching reference filenames.

    A domain matches if either:
    - at least `threshold` distinct keywords appear, OR
    - any single keyword appears at least `_DOMAIN_INSTANCE_THRESHOLD` times.
    """
    transcript_lower = transcript.lower()
    matched = []
    for filename, keywords in DOMAIN_KEYWORDS.items():
        threshold = _DOMAIN_MATCH_THRESHOLD_OVERRIDES.get(filename, _DOMAIN_MATCH_THRESHOLD)
        distinct_hits = 0
        max_single_count = 0
        for kw in keywords:
            count = transcript_lower.count(kw.lower())
            if count > 0:
                distinct_hits += 1
                if count > max_single_count:
                    max_single_count = count
        if distinct_hits >= threshold or max_single_count >= _DOMAIN_INSTANCE_THRESHOLD:
            matched.append(filename)
    return matched


def _load_reference(filename: str) -> Optional[str]:
    """Load a reference file from the references directory. Returns None if not found."""
    ref_path = _REFERENCES_DIR / filename
    try:
        return ref_path.read_text(encoding="utf-8")
    except (FileNotFoundError, OSError):
        return None


# Embedded clean-transcript instructions.
# Derived from the user's compressed, de-skilled clean-transcript reference;
# this form won a head-to-head tournament across top Ollama models where
# Gemma 4 (31b-cloud) decisively outperformed much larger models.
CLEAN_TRANSCRIPT_INSTRUCTIONS = """# Transcript-Cleaning Instructions

## Purpose

"Cleaning" a transcript means improving the formatting, removing chrome, adding punctuation where necessary, and **replacing obvious artifacts of inaccurate automated speech-to-text processing with the actual words that were spoken**

In other words, improving transcription fidelity without altering the substance of the transcript in any other way

The three cardinal sins of transcript cleaning to avoid at all costs:
1. **Summarization** - Condensing or shortening the original content
2. **Paraphrasing** - Rewording or rephrasing the original expressions
3. **Hallucination** - Adding content that wasn't in the original

### Step 1: Clean the Transcript

Most transcripts you handle will be either YouTube auto-captions or Whisper-derived audio transcripts

Start by noting the speaking style as a baseline
- Scan for potential transcription errors (natural artifacts of automated speech-to-text processing)
- Use context clues and phonetic reasoning to determine what the speaker *actually said*

Do not evaluate the claims of the speakers or speculate on the subject material itself

Apply the following transformations to the raw speech-to-text transcript you've been given:

#### Formatting
- Remove any YouTube auto-caption headers (e.g., "Kind: captions" / "Language: en" lines at the start)
- Combine fragmented lines into proper paragraphs
- Remove excessive newlines (transcripts often break every few words)
- Add paragraph breaks where natural topic shifts occur
- Avoid multiple speakers in the same paragraph
- Avoid "wall of text" paragraphs
- Maintain logical flow between sentences

#### Punctuation
- Add periods, commas, question marks, and other punctuation where appropriate
- Capitalize sentence beginnings
- Add quotation marks for quoted speech if present
- Remove speaker markers, `&gt;&gt;`, as they are highly unreliable -- Do not replace with `>>`
- Replace `[&nbsp;__&nbsp;]` (swear words) with `[__]`

#### Spoken Content Preservation

Since your job is strictly to restore the text to what was actually said by the speaker(s), you want to KEEP:
- Natural speech disfluencies: "um", "uh", "er", "ah", etc.
- False starts and self-corrections: "I went to—I mean, I drove to the store"
- Stutters: "I-I-I didn't know"
- Filler phrases: "like", "you know", "kind of", "sort of"

Where appropriate, format with commas for readability:
  - e.g., "I think, uh, we should go" (not "I think uh we should go")

**When In Doubt, Leave As Is**

If you are either
1) Not 100% certain a word/phrase is an error (e.g., it could be a proper noun, handle, or technical term)
2) Fairly certain a word/phrase is an error but *highly uncertain* about the ground truth of what was actually said

...then *DON'T make low-confidence guesses*. It's much easier for readers to recognize and internally correct for speech-to-text model flubs than inaccurate-but-fluent LLM-derived hallucinations.

### Critical constraints

- Do NOT paraphrase or rephrase the content
- Do NOT summarize or condense the content
- Do NOT add information that wasn't in the original
- Do NOT remove disfluent or filler speech patterns (e.g., "um", "uh", false starts, etc.)
- Do NOT replace "nonsensical" words if they might be jargon
- Do NOT merge multiple short sentences into one long sentence
- Do NOT "clean up" grammar

### Step 2: Output the cleaned transcript

Emit the cleaned transcript.

## Quality Standards

A well-cleaned transcript:
- Reads naturally with proper paragraphs and punctuation
- Maintains the same word count as the original (within 95-105%)
- Preserves what the speaker(s) said verbatim
- Has appropriate capitalization
- Formats disfluencies cleanly with commas for readability"""

# Sharding constants
OLLAMA_TOKEN_LIMIT = 2500  # Max tokens per shard for Ollama
TOKEN_ENCODING = "o200k_base"  # tiktoken encoding for all token counting

# Abbreviations to avoid splitting sentences after (literal strings, not regex)
ABBREVIATIONS = [
    "Dr", "Mr", "Mrs", "Ms", "Prof", "vs", "etc", "Jr", "Sr",
    "Inc", "Ltd", "St", "Ave", "Mt", "e.g", "i.e"
]

# Type alias for shard progress callback: (current_shard, total_shards) -> None
ShardProgressCallback = Callable[[int, int], None]

# Type alias for log/streaming callback: (text_chunk) -> None
LogCallback = Callable[[str], None]


@dataclass(frozen=True)
class WordCountValidation:
    original_count: int
    cleaned_count: int
    percentage: float
    lower_bound: int
    upper_bound: int
    is_valid: bool


def clean_transcript(
    raw_text: str,
    cleaner: str,
    *,
    verbose: bool = False,
    shard_callback: Optional[ShardProgressCallback] = None,
    log_callback: Optional[LogCallback] = None,
    stop_event: Optional[threading.Event] = None,
) -> Optional[str]:
    """
    Clean a transcript via Ollama.

    Claude cleaning is handled by the Rust backend, not this module.
    A "claude" selector reaching this function is treated as off.

    Args:
        raw_text: Raw transcript to clean
        cleaner: "off" or "ollama" (or an Ollama model name)
        verbose: Whether to log debug info
        shard_callback: Callback for shard progress (current_shard, total_shards)
        log_callback: Callback for streaming output (receives text chunks for verbose display)
        stop_event: Threading event to signal cancellation

    Returns:
        Cleaned transcript text, or None on failure / disabled
    """
    selection = _normalize_cleaner_selection(cleaner)
    if selection == "off" or selection == "claude":
        # Claude path is no longer routed here; orchestrator should never send it.
        return None

    # "ollama" or any unrecognized value routes to Ollama (backward compat)
    return clean_transcript_with_ollama(
        raw_text,
        verbose=verbose,
        shard_callback=shard_callback,
        log_callback=log_callback,
        stop_event=stop_event,
    )


def validate_word_count(original_text: str, cleaned_text: str) -> WordCountValidation:
    """Validate cleaned transcript length using the same rules as the skill script."""
    original_count = _count_words_from_text(original_text, is_cleaned=False)
    cleaned_count = _count_words_from_text(cleaned_text, is_cleaned=True)

    lower_bound = int(original_count * 0.95)
    upper_bound = int(original_count * 1.05)
    percentage = (cleaned_count / original_count * 100.0) if original_count > 0 else 0.0

    return WordCountValidation(
        original_count=original_count,
        cleaned_count=cleaned_count,
        percentage=percentage,
        lower_bound=lower_bound,
        upper_bound=upper_bound,
        is_valid=(lower_bound <= cleaned_count <= upper_bound),
    )


def clean_transcript_with_ollama(
    raw_text: str,
    verbose: bool = False,
    shard_callback: Optional[ShardProgressCallback] = None,
    log_callback: Optional[LogCallback] = None,
    stop_event: Optional[threading.Event] = None,
) -> Optional[str]:
    """
    Clean a transcript using Ollama (gemma4:31b-cloud).

    Supports sharding for large transcripts (>2500 tokens) and streaming output
    for verbose mode.

    Args:
        raw_text: Raw transcript text to clean
        verbose: If True, stream and print model output including <think> blocks
        shard_callback: Optional callback for shard progress (current, total)
        log_callback: Optional callback for streaming output (receives text chunks)
        stop_event: Threading event to signal cancellation

    Returns:
        Cleaned transcript text, or None on failure
    """
    if not raw_text or not raw_text.strip():
        if verbose:
            warnings.warn("Empty transcript provided. Nothing to clean.")
        return None

    # Programmatic cleanup before sending to model
    raw_text = preprocess_transcript(raw_text)

    shards = _split_into_shards(
        raw_text,
        verbose=verbose,
        token_limit=OLLAMA_TOKEN_LIMIT,
    )
    total_shards = len(shards)

    if verbose:
        print(f"Calling Ollama ({OLLAMA_MODEL})...")
        print(f"Transcript length: {len(raw_text)} chars")
        if total_shards > 1:
            print(f"Split into {total_shards} shards for processing")

    cleaned_parts: List[str] = []

    for shard_idx, shard_text in enumerate(shards, 1):
        if stop_event and stop_event.is_set():
            if verbose:
                print("Cancelled by user")
            return None

        if shard_callback and total_shards > 1:
            shard_callback(shard_idx, total_shards)

        prompt = _build_ollama_prompt(shard_text)
        transcript_tokens = count_tokens(shard_text)
        prompt_tokens = count_tokens(prompt)
        num_ctx = calculate_num_ctx(transcript_tokens, prompt_tokens)

        if verbose and total_shards > 1:
            print(f"Processing shard {shard_idx}/{total_shards} (num_ctx: {num_ctx}) ...")

        try:
            if verbose:
                cleaned_shard = ollama_generate_streaming(
                    OLLAMA_MODEL, prompt, num_ctx=num_ctx,
                    system_prompt=OLLAMA_SYSTEM_PROMPT,
                    log_callback=log_callback, stop_event=stop_event)
            else:
                cleaned_shard = ollama_generate(
                    OLLAMA_MODEL, prompt, num_ctx=num_ctx,
                    system_prompt=OLLAMA_SYSTEM_PROMPT,
                    stop_event=stop_event)

            if cleaned_shard:
                cleaned_parts.append(cleaned_shard)
            else:
                if verbose:
                    print(f"Shard {shard_idx} returned empty result")
                return None

        except Exception as e:
            if verbose:
                print(f"Ollama error on shard {shard_idx}: {e}")
            return None

    result = _concatenate_shards(cleaned_parts)
    return result if result.strip() else None


def _count_tokens_tiktoken(text: str) -> Optional[int]:
    """Count tokens using tiktoken o200k_base encoding."""
    if not TIKTOKEN_AVAILABLE:
        return None
    try:
        enc = tiktoken.get_encoding(TOKEN_ENCODING)
        return len(enc.encode(text))
    except Exception:
        return None


def _concatenate_shards(parts: List[str]) -> str:
    """
    Concatenate cleaned shards without adding empty newlines.
    
    Since shards are split at sentence boundaries, we join them directly.
    If the previous shard ends with sentence-ending punctuation, we add a space.
    Otherwise, we concatenate directly.
    """
    if not parts:
        return ""
    if len(parts) == 1:
        return parts[0]
    
    result = parts[0]
    for part in parts[1:]:
        if not part:
            continue
        # If previous ends with sentence punctuation, add a space before next
        if result and result[-1] in '.!?':
            result += " " + part
        else:
            result += part
    return result


def _split_into_shards(
    text: str,
    verbose: bool = False,
    token_limit: int = OLLAMA_TOKEN_LIMIT,
) -> List[str]:
    """
    Split text into N equal-sized shards if it exceeds the token limit.

    All token counting uses tiktoken o200k_base (v10: no more Anthropic API).
    Splits at sentence boundaries to avoid cutting mid-sentence.
    The number of shards is calculated dynamically: ceil(token_count / token_limit).

    Args:
        text: Text to split
        verbose: Whether to log debug info
        token_limit: Maximum tokens per shard
    """
    import math

    token_count = _count_tokens_tiktoken(text)
    if token_count is None:
        if verbose:
            warnings.warn("tiktoken o200k_base unavailable, processing as single shard")
        return [text]
    if verbose:
        print(f"Tokenizing transcript with {TOKEN_ENCODING}")
    
    if token_count <= token_limit:
        if verbose:
            print(f"Token count: {token_count} (under {token_limit} limit, no sharding needed)")
        return [text]
    
    # Calculate required number of shards dynamically
    num_shards = math.ceil(token_count / token_limit)
    
    if verbose:
        print(f"Token count: {token_count}, will create {num_shards} shards (limit: {token_limit})")
    
    # Split by sentences
    sentences = _split_into_sentences(text)
    
    if len(sentences) < num_shards:
        # Not enough sentences to create shards, just split equally by word count
        words = text.split()
        shard_size = len(words) // num_shards
        shards = []
        for i in range(num_shards):
            start = i * shard_size
            end = start + shard_size if i < num_shards - 1 else len(words)
            shards.append(" ".join(words[start:end]))
        return [s for s in shards if s.strip()]
    
    # Distribute sentences into N roughly equal shards by word count
    total_words = len(text.split())
    target_words_per_shard = total_words // num_shards
    
    shards: List[str] = []
    current_shard_sentences: List[str] = []
    current_word_count = 0
    
    for sentence in sentences:
        sentence_words = len(sentence.split())
        
        # Check if adding this sentence would exceed target and we have content
        # Also ensure we don't create more shards than needed
        if (current_word_count + sentence_words > target_words_per_shard 
            and current_shard_sentences 
            and len(shards) < num_shards - 1):
            # Finalize current shard
            shards.append(" ".join(current_shard_sentences))
            current_shard_sentences = [sentence]
            current_word_count = sentence_words
        else:
            current_shard_sentences.append(sentence)
            current_word_count += sentence_words
    
    # Add remaining sentences as the last shard
    if current_shard_sentences:
        shards.append(" ".join(current_shard_sentences))
    
    return [s for s in shards if s.strip()]


def _split_into_sentences(text: str) -> List[str]:
    """
    Split text into sentences at common sentence boundaries.
    
    Avoids splitting after common abbreviations like Dr., Mr., Mrs., Ms., Prof.,
    vs., etc., e.g., i.e., Jr., Sr., Inc., Ltd., St., Ave., Mt.
    
    Uses a placeholder approach to avoid variable-width lookbehind issues with
    Python's re module.
    """
    # Protect abbreviations by replacing their periods with a placeholder
    PLACEHOLDER = "\x00"  # Null character, unlikely to appear in transcripts
    protected_text = text
    
    for abbr in ABBREVIATIONS:
        # Match abbreviation followed by a period (case-insensitive for flexibility)
        # e.g. "Dr." -> "Dr\x00"
        protected_text = re.sub(
            rf'\b{re.escape(abbr)}\.',
            abbr + PLACEHOLDER,
            protected_text,
            flags=re.IGNORECASE
        )
    
    # Now split on sentence-ending punctuation followed by whitespace
    # This won't split on protected abbreviation periods (they have no '.' anymore)
    sentences_raw = re.split(r'(?<=[.!?])\s+', protected_text)
    
    # Restore the periods in abbreviations
    sentences = []
    for s in sentences_raw:
        restored = s.replace(PLACEHOLDER, '.')
        if restored.strip():
            sentences.append(restored.strip())
    
    return sentences


def ollama_generate(model: str, prompt: str, temperature: float = 1.0,
                    num_ctx: int = 32768, top_k: int = 64, top_p: float = 0.95,
                    repeat_penalty: float = 1.1, think: bool = True,
                    system_prompt: str = "",
                    stop_event: Optional[threading.Event] = None) -> str:
    """Call Ollama chat API and return the response text (no timeout)."""
    if stop_event and stop_event.is_set():
        return ""

    url = f"{OLLAMA_HOST}/api/chat"
    messages = []
    if system_prompt:
        messages.append({"role": "system", "content": system_prompt})
    messages.append({"role": "user", "content": prompt})
    payload = {
        "model": model,
        "messages": messages,
        "stream": False,
        "think": think,
        "options": {
            "temperature": temperature,
            "num_ctx": num_ctx,
            "top_k": top_k,
            "top_p": top_p,
            "repeat_penalty": repeat_penalty,
        },
    }

    body = json.dumps(payload).encode("utf-8")
    req = Request(url, data=body, headers={"Content-Type": "application/json"}, method="POST")

    try:
        with urlopen(req) as resp:
            data = json.loads(resp.read().decode("utf-8", errors="replace"))
    except HTTPError as e:
        detail = e.read().decode("utf-8", errors="replace") if hasattr(e, "read") else str(e)
        # If model doesn't support thinking, retry without
        if e.code == 400 and "does not support thinking" in detail and think:
            return ollama_generate(model, prompt, temperature, num_ctx, top_k, top_p,
                                   repeat_penalty, think=False,
                                   system_prompt=system_prompt, stop_event=stop_event)
        raise RuntimeError(f"HTTP {e.code}: {detail}") from e
    except URLError as e:
        raise RuntimeError(f"Connection error: {e}") from e

    message = data.get("message", {})
    content = message.get("content", "").strip()

    # Strip markdown code fences if model wrapped output
    lines = content.splitlines()
    if len(lines) >= 2 and lines[0].strip().startswith("```") and lines[-1].strip().startswith("```"):
        content = "\n".join(lines[1:-1]).strip()

    return content


def ollama_generate_streaming(model: str, prompt: str, temperature: float = 1.0,
                               num_ctx: int = 32768, top_k: int = 64, top_p: float = 0.95,
                               repeat_penalty: float = 1.1, think: bool = True,
                               system_prompt: str = "",
                               log_callback: Optional[LogCallback] = None,
                               stop_event: Optional[threading.Event] = None) -> str:
    """Call Ollama chat API with streaming to show progress."""
    url = f"{OLLAMA_HOST}/api/chat"
    messages = []
    if system_prompt:
        messages.append({"role": "system", "content": system_prompt})
    messages.append({"role": "user", "content": prompt})
    payload = {
        "model": model,
        "messages": messages,
        "stream": True,
        "think": think,
        "options": {
            "temperature": temperature,
            "num_ctx": num_ctx,
            "top_k": top_k,
            "top_p": top_p,
            "repeat_penalty": repeat_penalty,
        },
    }

    body = json.dumps(payload).encode("utf-8")
    req = Request(url, data=body, headers={"Content-Type": "application/json"}, method="POST")

    def _emit(text: str):
        if log_callback:
            log_callback(text)

    try:
        full_response = []
        in_thinking = False

        with urlopen(req) as resp:
            while True:
                if stop_event and stop_event.is_set():
                    _emit("[Cancelled]")
                    return ""

                line = resp.readline()

                if not line:
                    break
                try:
                    chunk = json.loads(line.decode("utf-8", errors="replace"))
                    message = chunk.get("message", {})

                    # Handle thinking tokens
                    if message.get("thinking"):
                        if not in_thinking:
                            _emit("[thinking...]")
                            in_thinking = True

                    # Handle content tokens
                    if message.get("content"):
                        if in_thinking:
                            _emit("[/thinking]")
                            in_thinking = False
                        content = message["content"]
                        full_response.append(content)
                        _emit(content)

                    if chunk.get("done"):
                        break
                except json.JSONDecodeError:
                    continue

        if in_thinking:
            _emit("[/thinking]")

        result = "".join(full_response).strip()
        # Strip markdown code fences
        lines = result.splitlines()
        if len(lines) >= 2 and lines[0].strip().startswith("```") and lines[-1].strip().startswith("```"):
            result = "\n".join(lines[1:-1]).strip()
        return result

    except HTTPError as e:
        detail = e.read().decode("utf-8", errors="replace") if hasattr(e, "read") else str(e)
        if e.code == 400 and "does not support thinking" in detail and think:
            _emit("[Model doesn't support thinking, retrying without]")
            return ollama_generate_streaming(model, prompt, temperature, num_ctx, top_k, top_p,
                                              repeat_penalty, think=False,
                                              system_prompt=system_prompt, log_callback=log_callback,
                                              stop_event=stop_event)
        raise RuntimeError(f"HTTP {e.code}: {detail}") from e
    except URLError as e:
        raise RuntimeError(f"Connection error: {e}") from e




def _normalize_cleaner_selection(cleaner: str) -> str:
    if cleaner is None:
        return "off"

    raw = str(cleaner).strip()
    if not raw:
        return "off"

    lowered = raw.lower()
    if lowered in {"0", "false", "no", "off", "none", "disabled"}:
        return "off"
    if lowered in {"1", "true", "yes", "on"}:
        # Back-compat with old bool config which meant "use Claude"
        return "claude"

    if lowered in {"claude", "anthropic"}:
        return "claude"

    # "ollama" or any unrecognized value (old model names, retired "gemini")
    # routes to Ollama
    return "ollama"


def preprocess_transcript(raw: str, strip_speaker_markers: bool = False) -> str:
    """
    Programmatic preprocessing to simplify the LLM's job:
    - Remove 'Kind: captions' and 'Language: en' header lines
    - Convert HTML entities: &gt;&gt; -> >>, &gt; -> >
    - Optionally strip >> speaker markers entirely (replaced by blank line)
    - Strip leading/trailing whitespace from lines
    - Collapse runs of 3+ blank lines to 2
    """
    lines = raw.splitlines()
    processed = []
    for line in lines:
        stripped = line.strip()
        # Remove header lines
        if re.match(r'^Kind:\s*captions\s*$', stripped, re.IGNORECASE):
            continue
        if re.match(r'^Language:\s*\w+\s*$', stripped, re.IGNORECASE):
            continue
        # Convert HTML entities
        stripped = stripped.replace('&gt;&gt;', '>>')
        stripped = stripped.replace('&gt;', '>')
        # Optionally strip >> markers
        if strip_speaker_markers:
            stripped = re.sub(r'^>>\s*', '', stripped)
        processed.append(stripped)

    text = '\n'.join(processed)
    # Collapse excessive blank lines
    text = re.sub(r'\n{3,}', '\n\n', text)
    return text.strip()


def _build_ollama_prompt(raw_text: str) -> str:
    """Build the Ollama prompt with full instructions and optional reference injection."""
    domains = _detect_domains(raw_text)

    reference_block = ""
    if domains:
        ref_parts = []
        for filename in domains:
            content = _load_reference(filename)
            if content:
                ref_parts.append(content.strip())
        if ref_parts:
            combined = "\n\n---\n\n".join(ref_parts)
            reference_block = f"""
<reference_material>
The following reference material contains correct spellings and names relevant to the domain discussed in this transcript. Use these as ground-truth when correcting mistranscriptions:

{combined}
</reference_material>

"""

    return f"""Please clean the attached transcript using the following instructions:

<instructions>
{CLEAN_TRANSCRIPT_INSTRUCTIONS}
</instructions>
{reference_block}
<transcript>
{raw_text}
</transcript>"""




# --- Word-count validation logic (mirrors scripts/validate_word_count.py) ---

LINE_FILLER_PATTERNS = [
    r"^Kind:\s*captions\s*$",
    r"^Language:\s*\w+\s*$",
    r"^\[music\]$",
    r"^\[applause\]$",
]

INLINE_STRIP_PATTERNS_ORIGINAL = [
    r"&gt;&gt;",
    r"&gt;",
]

INLINE_STRIP_PATTERNS_CLEANED = [
    r">>",
]


def _strip_filler(text: str, *, is_cleaned: bool) -> str:
    import re

    lines = text.split("\n")
    processed_lines = []

    for line in lines:
        line_stripped = line.strip()

        is_filler_line = False
        for pattern in LINE_FILLER_PATTERNS:
            if re.match(pattern, line_stripped, re.IGNORECASE):
                is_filler_line = True
                break

        if is_filler_line:
            continue

        if is_cleaned:
            for pattern in INLINE_STRIP_PATTERNS_CLEANED:
                line = re.sub(pattern, "", line)
        else:
            for pattern in INLINE_STRIP_PATTERNS_ORIGINAL:
                line = re.sub(pattern, "", line)

        processed_lines.append(line)

    return "\n".join(processed_lines)


def _count_words_from_text(text: str, *, is_cleaned: bool) -> int:
    stripped = _strip_filler(text or "", is_cleaned=is_cleaned)
    return len(stripped.split())
