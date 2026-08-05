interface ToolParameter {
  type: "string" | "number" | "integer" | "boolean" | "array" | "object";
  description: string;
  enum?: (string | number)[];
  items?: unknown;
}

export interface ToolDefinition {
  type: "function";
  function: {
    name: string;
    description: string;
    parameters: {
      type: "object";
      properties: Record<string, ToolParameter>;
      required?: string[];
    };
  };
}

export const BANKING_TOOL_DEFINITIONS: readonly ToolDefinition[] = [
  {
    type: "function",
    function: {
      name: "transfer_to_human_agents",
      description: "Transfer the user to a human agent.",
      parameters: {
        type: "object",
        properties: {
          summary: {
            type: "string",
            description:
              "A summary of the user's issue and what was attempted before transfer.",
          },
          reason: {
            type: "string",
            enum: [
              "fraud_or_security_concern",
              "account_closure_request",
              "deceased_account_holder",
              "legal_or_regulatory_matter",
              "account_ownership_dispute",
              "complex_billing_dispute",
              "abusive_customer_behavior",
              "third_party_inquiry",
              "technical_system_error",
              "unconfirmed_external_communication",
              "customer_demands_after_unavailable_offer_refusal",
              "kb_search_unsuccessful_customer_requests_transfer",
              "specialized_department_required",
              "accessibility_or_special_needs",
              "customer_frustrated_demands_human",
              "supervisor_request_service_complaint",
              "customer_requests_human_no_specific_reason",
              "request_completed_customer_wants_human_followup",
              "other",
            ],
            description: "The specific reason code for the transfer.",
          },
        },
        required: ["summary"],
      },
    },
  },
  {
    type: "function",
    function: {
      name: "get_current_time",
      description:
        "Get the current time. Use this to get the current timestamp for logging verification records.",
      parameters: {
        type: "object",
        properties: {},
      },
    },
  },
  {
    type: "function",
    function: {
      name: "get_user_information_by_id",
      description:
        "Get the information (date of birth, email, phone number, address) for a user by their user id.",
      parameters: {
        type: "object",
        properties: {
          user_id: {
            type: "string",
            description: "The ID of the user",
          },
        },
        required: ["user_id"],
      },
    },
  },
  {
    type: "function",
    function: {
      name: "get_user_information_by_name",
      description:
        "Get the information (date of birth, email, phone number, address) for a user by their name. Case Sensitive.",
      parameters: {
        type: "object",
        properties: {
          customer_name: {
            type: "string",
            description: "The name of the user",
          },
        },
        required: ["customer_name"],
      },
    },
  },
  {
    type: "function",
    function: {
      name: "get_user_information_by_email",
      description:
        "Get the information (date of birth, email, phone number, address) for a user by their email.",
      parameters: {
        type: "object",
        properties: {
          email: {
            type: "string",
            description: "The email of the user",
          },
        },
        required: ["email"],
      },
    },
  },
  {
    type: "function",
    function: {
      name: "change_user_email",
      description: "Change the email address for a user.",
      parameters: {
        type: "object",
        properties: {
          user_id: {
            type: "string",
            description: "The ID of the user whose email should be changed",
          },
          new_email: {
            type: "string",
            description: "The new email address to set for the user",
          },
        },
        required: ["user_id", "new_email"],
      },
    },
  },
  {
    type: "function",
    function: {
      name: "get_referrals_by_user",
      description: "Get all referrals made by a user.",
      parameters: {
        type: "object",
        properties: {
          user_id: {
            type: "string",
            description:
              "The ID of the user (referrer) to look up referrals for",
          },
        },
        required: ["user_id"],
      },
    },
  },
  {
    type: "function",
    function: {
      name: "get_credit_card_transactions_by_user",
      description: "Get all credit card transactions for a user.",
      parameters: {
        type: "object",
        properties: {
          user_id: {
            type: "string",
            description: "The ID of the user to look up transactions for",
          },
        },
        required: ["user_id"],
      },
    },
  },
  {
    type: "function",
    function: {
      name: "get_credit_card_accounts_by_user",
      description:
        "Get all credit card accounts for a user. Returns information about each credit card account including card type, date opened, current balance, and reward points.",
      parameters: {
        type: "object",
        properties: {
          user_id: {
            type: "string",
            description:
              "The ID of the user to look up credit card accounts for",
          },
        },
        required: ["user_id"],
      },
    },
  },
  {
    type: "function",
    function: {
      name: "log_verification",
      description:
        "Log a verification record after successfully verifying a user's identity. Call this tool after you have verified a user by confirming 2 out of 4 identity fields (date of birth, email, phone number, address). This creates an audit record of the verification.",
      parameters: {
        type: "object",
        properties: {
          name: {
            type: "string",
            description: "The verified user's full name",
          },
          user_id: {
            type: "string",
            description: "The verified user's ID",
          },
          address: {
            type: "string",
            description: "The verified user's address",
          },
          email: {
            type: "string",
            description: "The verified user's email",
          },
          phone_number: {
            type: "string",
            description: "The verified user's phone number",
          },
          date_of_birth: {
            type: "string",
            description:
              "The verified user's date of birth (MM/DD/YYYY format)",
          },
          time_verified: {
            type: "string",
            description:
              'The timestamp of the verification (e.g., "2025-11-14 03:40:00 EST")',
          },
        },
        required: [
          "name",
          "user_id",
          "address",
          "email",
          "phone_number",
          "date_of_birth",
          "time_verified",
        ],
      },
    },
  },
  {
    type: "function",
    function: {
      name: "give_discoverable_user_tool",
      description:
        'Pass a tool to the user so they can execute it themselves. Use this when the knowledge base indicates that the user should perform an action themselves (e.g., "to do X, have the user call tool_name(args)"). The user will then be able to call `call_discoverable_user_tool` with the same tool name and arguments to simulate executing the action.',
      parameters: {
        type: "object",
        properties: {
          discoverable_tool_name: {
            type: "string",
            description:
              'The name of the discoverable tool (e.g., "open_webpage", "navigate_to_section")',
          },
          arguments: {
            type: "string",
            description:
              'JSON string of arguments for the tool (e.g., \'{"url": "https://example.com"}\')',
          },
        },
        required: ["discoverable_tool_name"],
      },
    },
  },
  {
    type: "function",
    function: {
      name: "unlock_discoverable_agent_tool",
      description:
        "Unlock an agent discoverable tool that was found in the knowledge base. Use this when the knowledge base indicates that you have access to a specialized internal tool. The knowledge base will tell you the tool name to unlock. After unlocking, you can use the tool by calling `call_discoverable_agent_tool` with the tool name and required arguments.",
      parameters: {
        type: "object",
        properties: {
          agent_tool_name: {
            type: "string",
            description:
              'The name of the agent discoverable tool to unlock (e.g., "calculate_apr_adjustment_7842")',
          },
        },
        required: ["agent_tool_name"],
      },
    },
  },
  {
    type: "function",
    function: {
      name: "call_discoverable_agent_tool",
      description:
        "Call an agent discoverable tool that you have previously unlocked. Use this after unlocking a tool with `unlock_discoverable_agent_tool`. The knowledge base will tell you which tool to use and what arguments to provide.",
      parameters: {
        type: "object",
        properties: {
          agent_tool_name: {
            type: "string",
            description: "The name of the agent discoverable tool to call",
          },
          arguments: {
            type: "string",
            description:
              'JSON string of arguments for the tool (e.g., \'{"user_id": "abc123"}\')',
          },
        },
        required: ["agent_tool_name"],
      },
    },
  },
  {
    type: "function",
    function: {
      name: "list_discoverable_agent_tools",
      description:
        "List all agent discoverable tools that you have called. Use this to see what specialized tools you have used.",
      parameters: {
        type: "object",
        properties: {},
      },
    },
  },
];
