const log = `👤 User: lets make something

🤖 Supervisor: Analyzing request and preparing execution plan...
[THOUGHT] [Supervisor] The user said "lets make something" but hasn't specified what they want to build. I don't have enough context to route to a developer or create a plan. I need to ask for clarification about what project or feature they have in mind. This is a conversational/greeting-like situation where I should respond helpfully and ask for more details.

🤖 Assistant: The user said "lets make something" but hasn't specified what they want to build. I don't have enough context to route to a developer or create a plan. I need to ask for clarification about what project or feature they have in mind. This is a conversational/greeting-like situation where I should respond helpfully and ask for more details.`;

function parseLogToSteps(log) {
  const lines = log.split('\n');
  const steps = [];
  let assistantResponse = null;
  let currentStep = null;

  for (const line of lines) {
    const trimmed = line.trim();
    if (!trimmed) continue;

    if (trimmed.startsWith('[THOUGHT]')) {
      const match = trimmed.match(/^\[THOUGHT\]\s+\[([^\]]+)\]\s+(.*)$/);
      if (match) {
        currentStep = {
          id: 'step-thought',
          type: 'thought',
          agent: match[1],
          title: match[1] + ' Thought',
          content: match[2],
        };
        steps.push(currentStep);
      }
    } else if (trimmed.includes('🔧 Calling')) {
      // skip
    } else if (trimmed.startsWith('🤖 Assistant:')) {
      assistantResponse = trimmed.slice(13).trim();
      currentStep = null;
    } else if (trimmed.startsWith('🤖 Supervisor:')) {
      currentStep = null;
    } else if (trimmed.startsWith('👤 User:') || trimmed.startsWith('🧹 Session')) {
      currentStep = null;
    } else {
      if (assistantResponse !== null && currentStep === null) {
        assistantResponse += '\n' + trimmed;
      } else if (currentStep) {
        currentStep.content += (currentStep.content ? '\n' : '') + trimmed;
      }
    }
  }

  return { steps, assistantResponse };
}

console.log(parseLogToSteps(log));
