# 0003. Style-Scoped Reference Packs Under Style-Neutral Subject Identity

- **Status**: accepted

A `Subject` stores style-neutral structured identity (`card.yaml`: proportions, physical height, `PaletteLock`, `ExclusiveSurfaceMap`, positive weapon geometry) separately from its rendered `ReferencePack` sheets, which are scoped by `(subject_slug, style_slug)` under `library/subjects/<slug>/packs/<style_slug>/`.

Keeping a single global turnaround sheet per `Subject` causes severe outline and rendering bleed (for example, Franco-Belgian black ink outlines leaking into painterly dark-fantasy outputs) when cross-style generation passes mismatched sheets alongside target `Style` exemplars. Scoping packs by `Style` avoids cross-style bleed at the cost of generating or bootstrapping a target-style `ReferencePack` when a subject is first rendered in a new style.
