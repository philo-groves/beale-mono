// Record synthetic executor results through the same store methods as the host runner.
export function recordRunbookExecution(store, runbookId, runId, provenance = {}) {
  const cells = store.executionPlan(runbookId);
  const startedAt = new Date().toISOString();
  store.beginExecution(runbookId, runId, cells.map((cell) => cell.id), "localhost", undefined, provenance);
  for (const cell of cells) {
    store.beginCellExecution(runbookId, runId, cell.id, "localhost");
    store.completeCellExecution({
      id: runbookId, runId, cellId: cell.id, status: "succeeded", exitCode: 0,
      startedAt, completedAt: new Date().toISOString(), durationMs: 1, proofTarget: "localhost",
      stdout: "Example assertion passed.\n",
    });
  }
  store.completeExecution({
    id: runbookId, runId, status: "succeeded", startedAt,
    completedAt: new Date().toISOString(), durationMs: 1, proofTarget: "localhost",
  });
}
