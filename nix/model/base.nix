# Base module: defines option types for tables, relationships, operations, effects
{ lib, config, ... }:

let
  # Organization metadata describes the top-level entity the generated app belongs to.
  organizationType = lib.types.submodule {
    options = {
      name = lib.mkOption {
        type = lib.types.str;
        default = "OpenB2C";
        description = "Human-readable organization or product name.";
      };
      description = lib.mkOption {
        type = lib.types.str;
        default = "Generated OpenB2C organization";
        description = "Organization or product description.";
      };
    };
  };

  audienceType = lib.types.enum [ "anonymous" "customer" "staff" "service" "system" ];

  authRoleType = lib.types.submodule {
    options = {
      label = lib.mkOption { type = lib.types.str; };
      description = lib.mkOption { type = lib.types.str; default = ""; };
      audience = lib.mkOption {
        type = lib.types.enum [ "customer" "staff" "service" "system" ];
        description = "Identity audience this role belongs to.";
      };
      defaultScopes = lib.mkOption {
        type = lib.types.listOf lib.types.str;
        default = [];
        description = "Scopes normally issued for this role. Enforcement still consumes normalized scopes.";
      };
      internal = lib.mkOption {
        type = lib.types.bool;
        default = false;
        description = "Whether this role is intended for generated system/runtime use rather than end users.";
      };
    };
  };

  authType = lib.types.submodule {
    options = {
      password.enable = lib.mkEnableOption "password signup, login and revocable bearer sessions";
      roles = lib.mkOption {
        type = lib.types.attrsOf authRoleType;
        default = {
          customer = {
            label = "Customer";
            description = "Authenticated customer identity for self-service account and owned-resource access.";
            audience = "customer";
            defaultScopes = [];
            internal = false;
          };
          staff = {
            label = "Staff";
            description = "Authenticated staff/operator identity for administrative workflows.";
            audience = "staff";
            defaultScopes = [];
            internal = false;
          };
          service = {
            label = "Service";
            description = "User-bound API key or integration identity for service-to-service access.";
            audience = "service";
            defaultScopes = [];
            internal = false;
          };
          system = {
            label = "System";
            description = "Trusted local system execution with the explicit wildcard scope.";
            audience = "system";
            defaultScopes = [ "*" ];
            internal = true;
          };
        };
        description = "Role metadata for generated documentation and clients. Runtime authorization still uses scopes.";
      };
    };
  };

  presentationGroupType = lib.types.submodule {
    options = {
      label = lib.mkOption {
        type = lib.types.str;
        description = "Human-readable operation group label.";
      };
      description = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        description = "Optional operation group description.";
      };
      displayPriority = lib.mkOption {
        type = lib.types.nullOr lib.types.int;
        default = null;
        description = "Lower values appear earlier in generated operation displays.";
      };
    };
  };

  presentationType = lib.types.submodule {
    options = {
      groups = lib.mkOption {
        type = lib.types.attrsOf presentationGroupType;
        default = {};
        description = "Reusable operation group metadata for generated clients.";
      };
    };
  };

  auditCategoryType = lib.types.enum [ "data" "workflow" "security" "payment" "system" ];

  auditEntityType = lib.types.submodule {
    options = {
      operations = lib.mkOption {
        type = lib.types.listOf lib.types.str;
        default = [ "create" "update" "delete" ];
        description = "Entity operations that must be audit logged.";
      };
      category = lib.mkOption {
        type = auditCategoryType;
        default = "data";
        description = "Default audit category for this entity.";
      };
      reason = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        description = "Why this entity requires audit logging.";
      };
    };
  };

  auditType = lib.types.submodule {
    options = {
      entities = lib.mkOption {
        type = lib.types.attrsOf auditEntityType;
        default = {};
        description = "Entity-level audit logging requirements.";
      };
    };
  };

  seedValueType = lib.types.nullOr (lib.types.oneOf [
    lib.types.str
    lib.types.int
    lib.types.number
    lib.types.bool
  ]);

  seedRowType = lib.types.attrsOf seedValueType;

  seedType = lib.types.submodule {
    options = {
      catalog = lib.mkOption {
        default = {};
        description = "Demo products and nested prices, resolved by product code rather than database IDs.";
        type = lib.types.attrsOf (lib.types.submodule {
          options = {
            name = lib.mkOption { type = lib.types.str; };
            description = lib.mkOption { type = lib.types.nullOr lib.types.str; default = null; };
            currency = lib.mkOption { type = lib.types.strMatching "[A-Z]{3}"; default = config.currency; description = "Defaults to the application's currency."; };
            prices = lib.mkOption {
              default = [];
              type = lib.types.listOf (lib.types.submodule {
                options = {
                  amount = lib.mkOption { type = lib.types.ints.positive; description = "Amount in the currency's minor units."; };
                  interval = lib.mkOption { type = lib.types.enum [ "one_off" "month" "year" ]; default = "one_off"; };
                };
              });
            };
          };
        });
      };
      reference = lib.mkOption {
        type = lib.types.attrsOf (lib.types.listOf seedRowType);
        default = {};
        description = "Stable reference data inserted or updated during runtime bootstrap.";
      };
      fixtures = lib.mkOption {
        type = lib.types.attrsOf (lib.types.listOf seedRowType);
        default = {};
        description = "Example or demo rows generated separately and applied only when fixtures are enabled.";
      };
      applyFixturesByDefault = lib.mkOption {
        type = lib.types.bool;
        default = false;
        description = "Apply fixture seed data by default in non-production runtime environments.";
      };
    };
  };

  integrationEnvVarType = lib.types.submodule {
    options = {
      description = lib.mkOption {
        type = lib.types.str;
        default = "";
        description = "Human-readable environment variable purpose.";
      };
      requiredInProduction = lib.mkOption {
        type = lib.types.bool;
        default = true;
        description = "Whether production startup must require this variable when the integration is used.";
      };
      secret = lib.mkOption {
        type = lib.types.bool;
        default = false;
        description = "Whether this variable must be supplied through deployment secrets.";
      };
      example = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        description = "Non-secret example value for generated environment templates.";
      };
    };
  };

  integrationEndpointType = lib.types.submodule {
    options = {
      provider = lib.mkOption {
        type = lib.types.str;
        default = "custom";
        description = "Provider implementation used by the generated integration handler.";
      };
      description = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        description = "Short generated-operator description for this integration.";
      };
      env = lib.mkOption {
        type = lib.types.attrsOf integrationEnvVarType;
        default = {};
        description = "Environment variable contract for this integration.";
      };
    };
  };

  webhookSigningType = lib.types.submodule {
    options = {
      enabled = lib.mkOption {
        type = lib.types.bool;
        default = true;
        description = "Whether generated outbound webhook effects include signatures.";
      };
      algorithm = lib.mkOption {
        type = lib.types.str;
        default = "sha256";
        description = "Signature algorithm label used in webhook signature headers.";
      };
      payload = lib.mkOption {
        type = lib.types.str;
        default = "timestamp.body";
        description = "Canonical signing payload shape.";
      };
      signatureHeader = lib.mkOption {
        type = lib.types.str;
        default = "X-OpenB2C-Signature";
        description = "HTTP header carrying the outbound webhook signature.";
      };
      timestampHeader = lib.mkOption {
        type = lib.types.str;
        default = "X-OpenB2C-Timestamp";
        description = "HTTP header carrying the outbound webhook timestamp.";
      };
      toleranceSeconds = lib.mkOption {
        type = lib.types.int;
        default = 300;
        description = "Default maximum age accepted by the generated webhook verifier.";
      };
    };
  };

  webhookIntegrationType = lib.types.submodule {
    options = {
      provider = lib.mkOption {
        type = lib.types.str;
        default = "openb2c";
        description = "Provider implementation used by the generated webhook handler.";
      };
      description = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        description = "Short generated-operator description for this integration.";
      };
      env = lib.mkOption {
        type = lib.types.attrsOf integrationEnvVarType;
        default = {};
        description = "Environment variable contract for this integration.";
      };
      signing = lib.mkOption {
        type = webhookSigningType;
        default = {};
        description = "Outbound webhook signature contract.";
      };
    };
  };

  integrationsType = lib.types.submodule {
    options = {
      identityEmail = lib.mkOption {
        type = integrationEndpointType;
        default = {
          provider = "resend";
          description = "Production identity OTP email delivery.";
          env = {
            EMAIL_PROVIDER = {
              description = "Email provider used for identity OTP delivery. Use resend for production or fake for local provider tests.";
              requiredInProduction = false;
              example = "resend";
            };
            RESEND_API_KEY = {
              description = "Resend API key used to send production identity OTP emails.";
              secret = true;
            };
            EMAIL_FROM = {
              description = "Verified sender address for production identity OTP emails.";
              example = "OpenB2C <login@example.com>";
            };
            IDENTITY_OTP_SUBJECT = {
              description = "Optional subject line override for identity OTP emails.";
              requiredInProduction = false;
            };
            RESEND_EMAILS_URL = {
              description = "Optional Resend emails API endpoint override for tests or proxies.";
              requiredInProduction = false;
              example = "https://api.resend.com/emails";
            };
          };
        };
        description = "Identity challenge email integration metadata.";
      };
      emailEffects = lib.mkOption {
        type = integrationEndpointType;
        default = {
          provider = "webhook";
          description = "Generated email effect dispatch endpoint.";
          env.EMAIL_WEBHOOK_URL = {
            description = "Email provider dispatch endpoint used by generated email effects.";
            secret = true;
          };
        };
        description = "Email effect integration metadata.";
      };
      webhookEffects = lib.mkOption {
        type = webhookIntegrationType;
        default = {
          provider = "openb2c";
          description = "Generated outbound webhook effect dispatch.";
          env = {
            WEBHOOK_URL = {
              description = "Webhook dispatch endpoint used by generated webhook effects.";
              secret = true;
            };
            WEBHOOK_SIGNING_SECRET = {
              description = "Shared secret used to sign outbound OpenB2C webhook effects.";
              secret = true;
            };
            WEBHOOK_SIGNATURE_TOLERANCE_SECONDS = {
              description = "Maximum accepted age for OpenB2C webhook signatures when using the generated verifier.";
              requiredInProduction = false;
              example = "300";
            };
          };
          signing = {
            enabled = true;
            algorithm = "sha256";
            payload = "timestamp.body";
            signatureHeader = "X-OpenB2C-Signature";
            timestampHeader = "X-OpenB2C-Timestamp";
            toleranceSeconds = 300;
          };
        };
        description = "Outbound webhook effect integration metadata.";
      };
    };
  };

  # Structured reference to a table field. These are generated under
  # `config.refs.<table>.<field>` so policy and metadata can avoid stringly
  # references.
  fieldRefType = lib.types.submodule {
    options = {
      table = lib.mkOption { type = lib.types.str; };
      field = lib.mkOption { type = lib.types.str; };
      references = lib.mkOption { type = lib.types.nullOr lib.types.str; default = null; };
    };
  };

  # Structured reference to an entity/table. These are generated under
  # `config.entityRefs.<table>` for policy that needs an entity target.
  tableRefType = lib.types.submodule {
    options = {
      table = lib.mkOption { type = lib.types.str; };
    };
  };

  fieldMetadataType = lib.types.submodule {
    options = {
      label = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        description = "Human-readable field label for generated API descriptions.";
      };
      helpText = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        description = "Short helper text shown beside generated inputs.";
      };
      placeholder = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        description = "Generated input placeholder.";
      };
      format = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        description = "Semantic field format such as email, date, time, url, money, or textarea.";
      };
      displayPriority = lib.mkOption {
        type = lib.types.nullOr lib.types.int;
        default = null;
        description = "Lower values appear earlier in generated forms, lists, and detail views.";
      };
      privacy = lib.mkOption {
        type = lib.types.enum [ "public" "internal" "sensitive" "secret" ];
        default = "public";
        description = "Field privacy classification for generated clients and API metadata.";
      };
      redact = lib.mkOption {
        type = lib.types.bool;
        default = false;
        description = "Whether generated API responses should omit this field.";
      };
    };
  };

  derivedFieldType = lib.types.submodule {
    options = {
      type = lib.mkOption {
        type = lib.types.str;
        default = "text";
        description = "Generated Rust/OpenAPI type for the derived value.";
      };
      metadata = lib.mkOption {
        type = fieldMetadataType;
        default = {};
        description = "Presentation metadata for the derived display-only value.";
      };
      dependencies = lib.mkOption {
        type = lib.types.listOf fieldRefType;
        default = [];
        description = "Structured local fields used to compute this value.";
      };
      template = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        description = "String template using {field_name} placeholders.";
      };
      expression = lib.mkOption {
        type = lib.types.nullOr lib.types.attrs;
        default = null;
        description = "Expression AST used to compute the value.";
      };
    };
  };

  fieldRelationshipType = lib.types.submodule {
    options = {
      label = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        description = "Human-readable relationship label for generated API descriptions.";
      };
      description = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        description = "Short explanation of the relationship.";
      };
      cardinality = lib.mkOption {
        type = lib.types.enum [ "one" "many" ];
        default = "one";
        description = "Relationship cardinality from the source row to the target entity.";
      };
    };
  };

  columnBoundsConstraintType = lib.types.submodule {
    options = {
      min = lib.mkOption {
        type = lib.types.nullOr lib.types.number;
        default = null;
        description = "Minimum accepted value.";
      };
      max = lib.mkOption {
        type = lib.types.nullOr lib.types.number;
        default = null;
        description = "Maximum accepted value.";
      };
    };
  };

  columnConstraintType = lib.types.submodule {
    options = {
      enum = lib.mkOption {
        type = lib.types.listOf lib.types.str;
        default = [];
        description = "Allowed finite values for this field.";
      };
      pattern = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        description = "JavaScript-compatible regular expression pattern for this field.";
      };
      length = lib.mkOption {
        type = columnBoundsConstraintType;
        default = {};
        description = "String length bounds for this field.";
      };
      range = lib.mkOption {
        type = columnBoundsConstraintType;
        default = {};
        description = "Numeric bounds for this field.";
      };
    };
  };

  # Column definition
  columnType = lib.types.submodule {
    options = {
      type = lib.mkOption { type = lib.types.str; };
      pk = lib.mkOption { type = lib.types.bool; default = false; };
      auto = lib.mkOption { type = lib.types.bool; default = false; };
      required = lib.mkOption { type = lib.types.bool; default = false; };
      unique = lib.mkOption { type = lib.types.bool; default = false; };
      default = lib.mkOption { type = lib.types.nullOr lib.types.str; default = null; };
      references = lib.mkOption { type = lib.types.nullOr lib.types.str; default = null; };
      metadata = lib.mkOption {
        type = fieldMetadataType;
        default = {};
        description = "Presentation, ordering, and privacy metadata for generated outputs.";
      };
      constraints = lib.mkOption {
        type = columnConstraintType;
        default = {};
        description = "Single-field constraints colocated with this column.";
      };
      relationship = lib.mkOption {
        type = lib.types.nullOr fieldRelationshipType;
        default = null;
        description = "Optional metadata for a foreign-key relationship represented by this column.";
      };
    };
  };

  # Record-level relationship between auth.userId and a resource field.
  relationshipType = lib.types.submodule {
    options = {
      field = lib.mkOption {
        type = fieldRefType;
        description = "Structured field reference implementing this relationship.";
      };
    };
  };

  relationshipSpecType = lib.types.either lib.types.str relationshipType;

  uniqueConstraintType = lib.types.submodule {
    options = {
      fields = lib.mkOption {
        type = lib.types.listOf fieldRefType;
        default = [];
        description = "Structured local field references that must be unique together.";
      };
      condition = lib.mkOption {
        type = lib.types.nullOr lib.types.attrs;
        default = null;
        description = "Optional Boolean expression AST limiting when the constraint applies.";
      };
      enforcement = lib.mkOption {
        type = lib.types.enum [ "auto" "database" "application" "both" ];
        default = "auto";
        description = "Preferred enforcement layer. Auto chooses database for unique constraints.";
      };
    };
  };

  checkConstraintType = lib.types.submodule {
    options = {
      fields = lib.mkOption {
        type = lib.types.listOf fieldRefType;
        default = [];
        description = "Structured local field references covered by this check.";
      };
      expression = lib.mkOption {
        type = lib.types.attrs;
        description = "Boolean expression AST that must evaluate truthy for a valid record.";
      };
      condition = lib.mkOption {
        type = lib.types.nullOr lib.types.attrs;
        default = null;
        description = "Optional Boolean expression AST limiting when the check applies.";
      };
      enforcement = lib.mkOption {
        type = lib.types.enum [ "auto" "database" "application" "both" ];
        default = "auto";
        description = "Preferred enforcement layer. Auto chooses database and application for check constraints.";
      };
    };
  };

  constraintGroupType = lib.types.submodule {
    options = {
      unique = lib.mkOption {
        type = lib.types.attrsOf uniqueConstraintType;
        default = {};
        description = "Uniqueness constraints keyed by logical constraint name.";
      };
      check = lib.mkOption {
        type = lib.types.attrsOf checkConstraintType;
        default = {};
        description = "Row-local check constraints keyed by logical constraint name.";
      };
    };
  };

  effectEventContractType = lib.types.submodule {
    options = {
      label = lib.mkOption { type = lib.types.nullOr lib.types.str; default = null; };
    };
  };

  notificationTemplateContractType = lib.types.submodule {
    options = {
      channel = lib.mkOption { type = lib.types.str; default = "email"; };
      label = lib.mkOption { type = lib.types.nullOr lib.types.str; default = null; };
    };
  };

  notificationRecipientContractType = lib.types.submodule {
    options = {
      label = lib.mkOption { type = lib.types.nullOr lib.types.str; default = null; };
    };
  };

  callActionContractType = lib.types.submodule {
    options = {
      label = lib.mkOption { type = lib.types.nullOr lib.types.str; default = null; };
    };
  };

  effectContractsType = lib.types.submodule {
    options = {
      events = lib.mkOption {
        type = lib.types.attrsOf effectEventContractType;
        default = {};
      };
      notifications = lib.mkOption {
        type = lib.types.submodule {
          options = {
            templates = lib.mkOption {
              type = lib.types.attrsOf notificationTemplateContractType;
              default = {};
            };
            recipients = lib.mkOption {
              type = lib.types.attrsOf notificationRecipientContractType;
              default = {};
            };
          };
        };
        default = {};
      };
      calls = lib.mkOption {
        type = lib.types.attrsOf (lib.types.attrsOf callActionContractType);
        default = {};
      };
    };
  };

  effectEventRefType = lib.types.submodule {
    options.event = lib.mkOption { type = lib.types.str; };
  };

  notificationTemplateRefType = lib.types.submodule {
    options = {
      template = lib.mkOption { type = lib.types.str; };
      channel = lib.mkOption { type = lib.types.str; };
    };
  };

  notificationRecipientRefType = lib.types.submodule {
    options.recipient = lib.mkOption { type = lib.types.str; };
  };

  callRefType = lib.types.submodule {
    options = {
      service = lib.mkOption { type = lib.types.str; };
      action = lib.mkOption { type = lib.types.str; };
    };
  };

  # Effect types
  effectType = lib.types.submodule {
    options = {
      emit = lib.mkOption { type = lib.types.nullOr effectEventRefType; default = null; };
      notify = lib.mkOption {
        type = lib.types.nullOr (lib.types.submodule {
          options = {
            template = lib.mkOption { type = notificationTemplateRefType; };
            to = lib.mkOption { type = notificationRecipientRefType; };
          };
        });
        default = null;
      };
      call = lib.mkOption {
        type = lib.types.nullOr callRefType;
        default = null;
      };
    };
  };

  assignmentValueType = lib.types.oneOf [
    lib.types.str
    lib.types.int
    lib.types.number
    lib.types.bool
  ];

  # Typed field assignment. Exactly one source is validated by codegen.
  assignmentType = lib.types.submodule {
    options = {
      value = lib.mkOption {
        type = lib.types.nullOr assignmentValueType;
        default = null;
        description = "Literal value to assign.";
      };
      now = lib.mkOption {
        type = lib.types.bool;
        default = false;
        description = "Assign the current timestamp.";
      };
      expression = lib.mkOption {
        type = lib.types.nullOr lib.types.attrs;
        default = null;
        description = "Expression AST used to compute the assigned value.";
      };
      input = lib.mkOption {
        type = lib.types.bool;
        default = false;
        description = "Assign from the operation input payload.";
      };
      clear = lib.mkOption {
        type = lib.types.bool;
        default = false;
        description = "Assign NULL.";
      };
    };
  };

  assignmentSetType = lib.types.attrsOf assignmentType;

  # Cascade update
  cascadeType = lib.types.submodule {
    options = {
      entity = lib.mkOption { type = tableRefType; };
      via = lib.mkOption {
        type = lib.types.nullOr lib.types.attrs;
        default = null;
      };
      set = lib.mkOption {
        type = assignmentSetType;
        default = {};
      };
    };
  };

  # Operation definition
  operationTransitionType = lib.types.submodule {
    options = {
      field = lib.mkOption {
        type = fieldRefType;
        description = "State field affected by the transition.";
      };
      from = lib.mkOption {
        type = lib.types.listOf lib.types.str;
        default = [];
        description = "Allowed source values for the transition.";
      };
      to = lib.mkOption {
        type = lib.types.str;
        description = "Target value after the transition.";
      };
    };
  };

  operationConfirmationType = lib.types.submodule {
    options = {
      required = lib.mkOption {
        type = lib.types.bool;
        default = false;
        description = "Whether generated clients should require explicit confirmation.";
      };
      title = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        description = "Confirmation dialog title.";
      };
      message = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        description = "Confirmation dialog message.";
      };
      confirmLabel = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        description = "Confirmation action label.";
      };
      severity = lib.mkOption {
        type = lib.types.enum [ "info" "warning" "danger" ];
        default = "warning";
        description = "Confirmation severity hint.";
      };
    };
  };

  operationPresentationType = lib.types.submodule {
    options = {
      label = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        description = "Human-readable operation label.";
      };
      description = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        description = "Short operation description for generated clients and API docs.";
      };
      group = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        description = "Presentation group key from presentation.groups.";
      };
      displayPriority = lib.mkOption {
        type = lib.types.nullOr lib.types.int;
        default = null;
        description = "Lower values appear earlier in generated operation lists.";
      };
      confirmation = lib.mkOption {
        type = operationConfirmationType;
        default = {};
        description = "Generated client confirmation requirements.";
      };
    };
  };

  operationPolicyType = lib.types.submodule {
    options = {
      audiences = lib.mkOption {
        type = lib.types.listOf audienceType;
        default = [];
        description = "Intended identity audiences. Empty means codegen derives audiences from public and relationship policy.";
      };
      risk = lib.mkOption {
        type = lib.types.enum [ "low" "medium" "high" ];
        default = "medium";
        description = "Operational risk hint for generated clients.";
      };
    };
  };

  operationAuditType = lib.types.submodule {
    options = {
      required = lib.mkOption {
        type = lib.types.bool;
        default = false;
        description = "Whether this specific operation must be audit logged.";
      };
      category = lib.mkOption {
        type = auditCategoryType;
        default = "data";
        description = "Audit category for this operation.";
      };
      reason = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        description = "Why this operation requires audit logging.";
      };
      summary = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        description = "Short audit text for this operation.";
      };
      detail = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        description = "Optional longer audit detail template.";
      };
    };
  };

  operationType = lib.types.submodule {
    options = {
      enabled = lib.mkOption {
        type = lib.types.bool;
        default = true;
        description = "Expose and permit this operation. False disables it for every caller, including system access.";
      };
      guard = lib.mkOption {
        type = lib.types.nullOr lib.types.attrs;  # Expression AST
        default = null;
        description = "Precondition expression (built with lib/expr.nix)";
      };
      relationships = lib.mkOption {
        type = lib.types.nullOr (lib.types.listOf relationshipSpecType);
        default = null;
        description = "Record relationships that permit this operation. Strings resolve by convention to <relationship>_id fields.";
      };
      public = lib.mkOption {
        type = lib.types.bool;
        default = false;
        description = "Allow this operation without an authenticated scope.";
      };
      scope = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        description = "Optional permission scope override. Defaults to <entity>.<operation>.";
      };
      policy = lib.mkOption {
        type = operationPolicyType;
        default = {};
        description = "Access and risk policy metadata.";
      };
      presentation = lib.mkOption {
        type = operationPresentationType;
        default = {};
        description = "Generated client and documentation presentation metadata.";
      };
      audit = lib.mkOption {
        type = operationAuditType;
        default = {};
        description = "Audit logging requirement metadata for this operation.";
      };
      transitions = lib.mkOption {
        type = lib.types.listOf operationTransitionType;
        default = [];
        description = "Allowed record state transitions performed by this operation.";
      };
      set = lib.mkOption {
        type = assignmentSetType;
        default = {};
        description = "Fields to update using typed assignment sources.";
      };
      cascade = lib.mkOption {
        type = lib.types.listOf cascadeType;
        default = [];
        description = "Cascading updates to related entities";
      };
      effects = lib.mkOption {
        type = lib.types.listOf effectType;
        default = [];
        description = "Side effects to trigger";
      };
    };
  };

in {
  imports = [
    (lib.mkAliasOptionModule [ "name" ] [ "organization" "name" ])
    (lib.mkAliasOptionModule [ "description" ] [ "organization" "description" ])
    (lib.mkAliasOptionModule [ "demo" "products" ] [ "seed" "catalog" ])
  ];
  config.seed.applyFixturesByDefault = lib.mkDefault (config.seed.catalog != {});
  options = {
    currency = lib.mkOption {
      type = lib.types.nullOr (lib.types.strMatching "[A-Z]{3}");
      default = null;
      description = "Default currency for catalog prices and demo products. Set this for applications that sell products.";
    };
    capabilities = lib.mkOption {
      default = {};
      description = "Explicit, versioned capability contracts and dependencies.";
      type = lib.types.attrsOf (lib.types.submodule {
        options = {
          version = lib.mkOption { type = lib.types.int; default = 1; };
          requires = lib.mkOption { type = lib.types.listOf lib.types.str; default = []; };
          entities = lib.mkOption { type = lib.types.attrsOf lib.types.str; default = {}; };
          settings = lib.mkOption { type = lib.types.attrs; default = {}; };
        };
      });
    };
    organization = lib.mkOption {
      type = organizationType;
      default = {};
      description = "Top-level organization or product metadata.";
    };

    auth = lib.mkOption {
      type = authType;
      default = {};
      description = "Authentication role and policy metadata.";
    };

    presentation = lib.mkOption {
      type = presentationType;
      default = {};
      description = "Presentation metadata for generated clients.";
    };

    audit = lib.mkOption {
      type = auditType;
      default = {};
      description = "Audit logging requirement metadata.";
    };

    seed = lib.mkOption {
      type = seedType;
      default = {};
      description = "Reference and fixture seed data.";
    };

    integrations = lib.mkOption {
      type = integrationsType;
      default = {};
      description = "System-level external integration configuration metadata.";
    };

    effectContracts = lib.mkOption {
      type = effectContractsType;
      default = {};
      description = "Declared effect events, notifications, recipients, and service actions.";
    };

    tables = lib.mkOption {
      type = lib.types.attrsOf (lib.types.attrsOf columnType);
      default = {};
      description = "Database table definitions";
    };

    derived = lib.mkOption {
      type = lib.types.attrsOf (lib.types.attrsOf derivedFieldType);
      default = {};
      description = "Display-only fields derived from stored entity values.";
    };

    refs = lib.mkOption {
      type = lib.types.attrsOf (lib.types.attrsOf fieldRefType);
      default = lib.mapAttrs (table: cols:
        lib.mapAttrs (field: col: {
          inherit table field;
          references = col.references;
        }) cols
      ) config.tables;
      description = "Structured references for table fields.";
    };

    entityRefs = lib.mkOption {
      type = lib.types.attrsOf tableRefType;
      default = lib.mapAttrs (table: _cols: { inherit table; }) config.tables;
      description = "Structured references for entities/tables.";
    };

    values = lib.mkOption {
      type = lib.types.attrsOf (lib.types.attrsOf (lib.types.attrsOf lib.types.str));
      default = lib.mapAttrs (table: cols:
        lib.mapAttrs (field: col:
          lib.listToAttrs (map (value: { name = value; inherit value; }) col.constraints.enum)
        ) cols
      ) config.tables;
      description = "Structured references for finite field values declared by column enum constraints.";
    };

    effectRefs = lib.mkOption {
      type = lib.types.attrs;
      default = {
        events = lib.mapAttrs (event: _contract: { inherit event; }) config.effectContracts.events;
        notifications = {
          templates = lib.mapAttrs (template: contract: {
            inherit template;
            channel = contract.channel;
          }) config.effectContracts.notifications.templates;
          recipients = lib.mapAttrs (recipient: _contract: { inherit recipient; }) config.effectContracts.notifications.recipients;
        };
        calls = lib.mapAttrs (service: actions:
          lib.mapAttrs (action: _contract: { inherit service action; }) actions
        ) config.effectContracts.calls;
      };
      description = "Structured references for declared effects.";
    };

    relationships = lib.mkOption {
      type = lib.types.attrsOf (lib.types.attrsOf relationshipType);
      default = {};
      description = "Record relationship definitions per entity.";
    };

    constraints = lib.mkOption {
      type = lib.types.attrsOf constraintGroupType;
      default = {};
      description = "Logical data constraints per entity, grouped by constraint type. Generators allocate enforcement to database and/or application layers.";
    };

    operations = lib.mkOption {
      type = lib.types.attrsOf (lib.types.attrsOf operationType);
      default = {};
      description = "Operations per entity (e.g., operations.ticket.confirm)";
    };

  };
}
