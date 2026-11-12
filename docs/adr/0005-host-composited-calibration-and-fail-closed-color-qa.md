# 0005. Host-Composited Calibration Strips and Fail-Closed Whole-Image Color QA

- **Status**: accepted

`imgforge` composites the `#252830` neutral calibration strip (`L*` gray wedge + exact `PaletteLock` swatches) deterministically on the host onto `ReferencePack` sheets rather than asking `image_gen` to draw hex swatches, slices components by foreground bounding-box detection (with grid priors and UI overrides), and treats whole-image OKLab/`DeltaE_2000` drift detectors strictly as fail/flag QA gates (never silently re-grading whole images outside explicit `export --match-grade`).

Generative models alter unshaded swatch RGB values and shift element positions across outputs, breaking blind pixel slicing and downstream color checks. Meanwhile, automatic global color transfer on whole scenes often washes out intentional atmospheric lighting; keeping `PatchEdit` local harmonization automatic while gating whole-image drift explicitly preserves both color integrity and human editorial control.
