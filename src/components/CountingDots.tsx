/** A live ellipsis: three dots in a small wave, sitting on the text baseline. */
export function CountingDots({ label = "Still counting" }: { label?: string }) {
  return (
    <span className="counting-dots" role="status" aria-label={label}>
      <span />
      <span />
      <span />
    </span>
  );
}
