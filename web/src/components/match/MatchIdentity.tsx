/**
 * Store label over the listing title in the display face. Shared by the
 * detail panel and the assistant's suggestion cards.
 */
export function MatchIdentity({
  store,
  title,
  titleId,
  level,
}: {
  store: string;
  title: string;
  titleId?: string;
  /** Heading level of the title within its surface. */
  level: "h2" | "h3";
}) {
  const Heading = level;
  return (
    <div className="match__identity">
      <p className="match__store">{store}</p>
      <Heading id={titleId} className="match__title">
        {title}
      </Heading>
    </div>
  );
}
