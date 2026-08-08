// Coarse-pointer pass for the touch shield contract.
//
// The main browser contract runs as a desktop pointer, where the shield is
// inert by design. Without this second emulated pass none of the touch surface
// is exercised at all, which is how a whole class of touch defects reached a
// phone unnoticed.

/**
 * Re-run the shield contract with a coarse primary pointer. The shield only
 * exists for touch, and the main pass runs as a desktop pointer, so without a
 * second emulated pass none of it is exercised — which is how a whole class of
 * touch defects shipped unnoticed.
 */
export async function auditTouchShield(page) {
  await page.send("Emulation.setDeviceMetricsOverride", {
    width: 554, height: 543, deviceScaleFactor: 2.55, mobile: true,
  });
  await page.send("Emulation.setTouchEmulationEnabled", { enabled: true, maxTouchPoints: 5 });
  await page.send("Emulation.setEmulatedMedia", {
    features: [
      { name: "prefers-reduced-motion", value: "reduce" },
      { name: "pointer", value: "coarse" },
      { name: "any-pointer", value: "coarse" },
    ],
  });
  const response = await page.send("Runtime.evaluate", {
    expression: "window.__runTouchShieldSuite()",
    awaitPromise: true,
    returnByValue: true,
  });
  if (response.exceptionDetails) {
    throw new Error(`touch shield contract threw: ${response.exceptionDetails.exception?.description ?? ""}`);
  }
  const outcome = response.result.value ?? {};
  if (outcome.failures?.length) {
    throw new Error(`touch shield contract failed: ${outcome.failures.join(" | ")}`);
  }
  if (!outcome.shieldAssertions || !outcome.handleAssertions || !outcome.softKeyboardAssertions) {
    throw new Error(
      `coarse-pointer pass did not run every contract: ${JSON.stringify(outcome)}`,
    );
  }
  return outcome;
}
