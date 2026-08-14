const budget = document.querySelector("#budget");
const reserve = document.querySelector("#reserve");
const budgetOutput = document.querySelector("#budget-output");
const reserveOutput = document.querySelector("#reserve-output");
const runButton = document.querySelector("#run-demo");
const runNote = document.querySelector("#run-note");
const transcript = document.querySelector("#transcript");
const terminalState = document.querySelector("#terminal-state");
const revealCard = document.querySelector("#reveal-card");
const agreedAmount = document.querySelector("#agreed-amount");
const paidAmount = document.querySelector("#paid-amount");

const formatAmount = (value) => Number(value).toLocaleString("en-US");
const wait = (milliseconds) => new Promise((resolve) => setTimeout(resolve, milliseconds));

function updateOutputs() {
  budgetOutput.value = formatAmount(budget.value);
  reserveOutput.value = formatAmount(reserve.value);
}

budget.addEventListener("input", updateOutputs);
reserve.addEventListener("input", updateOutputs);

async function addLine(index, message) {
  const item = document.createElement("li");
  item.innerHTML = `<span>${String(index).padStart(2, "0")}</span>${message}`;
  transcript.append(item);
  requestAnimationFrame(() => item.classList.add("visible"));
  await wait(330);
}

runButton.addEventListener("click", async () => {
  const buyerBudget = Number(budget.value);
  const sellerReserve = Number(reserve.value);

  runButton.disabled = true;
  revealCard.hidden = true;
  transcript.replaceChildren();
  terminalState.textContent = "RUNNING";
  runNote.textContent = "Applying the same policy decisions as the Python reference agents.";

  await addLine(1, "opened encrypted channel <strong>ch_0xbuyer_0xseller</strong>");
  await addLine(2, `buyer proposed <strong>${formatAmount(buyerBudget)}</strong> token units`);

  if (buyerBudget < sellerReserve) {
    await addLine(3, `seller rejected: reserve is <strong>${formatAmount(sellerReserve)}</strong>`);
    await addLine(4, "negotiation ended without settlement");
    terminalState.textContent = "NO DEAL";
    runNote.textContent = "Raise the buyer budget or lower the seller reserve to settle.";
  } else {
    await addLine(3, `seller countered at <strong>${formatAmount(buyerBudget)}</strong> token units`);
    await addLine(4, "buyer accepted the counteroffer");
    await addLine(5, "accepted offer and shielded payment committed atomically");
    await addLine(6, "viewing key granted to <strong>0xauditor</strong>");
    await addLine(7, "auditor reconstructed two offers and the settlement record");

    agreedAmount.textContent = formatAmount(buyerBudget);
    paidAmount.textContent = formatAmount(buyerBudget);
    revealCard.hidden = false;
    terminalState.textContent = "SETTLED";
    runNote.textContent = "Simulation complete. The evidence section links to the existing Sepolia run.";
  }

  runButton.disabled = false;
});

updateOutputs();
