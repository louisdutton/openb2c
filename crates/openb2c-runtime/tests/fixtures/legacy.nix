# Historical schema fixture for identity, migration and SQL compatibility tests.
{lib}: let
  composeLib = import ../../../../nix/model/lib/compose.nix {inherit lib;};

  modules = lib.evalModules {
    modules = [
      ../../../../nix/model/base.nix
      ({config, ...}: {
        organization = {
          name = "Duchy Opera";
          description = "The Cornish Opera Company";
        };
        seed = {
          applyFixturesByDefault = true;
          reference.venue = [
            {
              id = 1;
              name = "Hall for Cornwall";
              address = "Back Quay";
              city = "Truro";
              postcode = "TR1 2LL";
              capacity = 900;
              active = 1;
            }
          ];
          fixtures = {
            user = [
              {
                id = 1;
                email = "ada@example.test";
                name = "Ada Lovelace";
                customer_type = "patron";
              }
              {
                id = 2;
                email = "elowen@example.test";
                name = "Elowen Trevorrow";
                customer_type = "individual";
              }
              {
                id = 3;
                email = "morgan@example.test";
                name = "Morgan Trelawny";
                customer_type = "individual";
              }
            ];
            artist_profile = [
              {
                id = 1;
                user_id = 2;
                role = "Soprano";
                bio = "Cornish soprano performing as Pamina.";
                active = 1;
              }
              {
                id = 2;
                user_id = 3;
                role = "Conductor";
                bio = "Music director for the Duchy Opera summer season.";
                active = 1;
              }
            ];
            production = [
              {
                id = 1;
                name = "The Magic Flute";
                season = "Summer 2026";
                description = "Duchy Opera pilot production for internal scheduling and public ticketing.";
                status = "active";
              }
            ];
            production_role = [
              {
                id = 1;
                production_id = 1;
                name = "Pamina";
                category = "cast";
                description = "Principal soprano role.";
                required_count = 1;
                status = "filled";
              }
              {
                id = 2;
                production_id = 1;
                name = "Music director";
                category = "creative";
                description = "Conductor and music preparation lead.";
                required_count = 1;
                status = "filled";
              }
            ];
            production_member = [
              {
                id = 1;
                production_id = 1;
                user_id = 2;
                role_id = 1;
                responsibility = "Pamina";
                status = "confirmed";
              }
              {
                id = 2;
                production_id = 1;
                user_id = 3;
                role_id = 2;
                responsibility = "Conductor";
                status = "confirmed";
              }
            ];
            rehearsal = [
              {
                id = 1;
                production_id = 1;
                name = "Act I music call";
                venue_id = 1;
                starts_at = "2026-06-29T18:30:00Z";
                ends_at = "2026-06-29T21:30:00Z";
                status = "published";
                notes = "Cover Pamina entrance and Act I finale.";
              }
            ];
            rehearsal_call = [
              {
                id = 1;
                rehearsal_id = 1;
                user_id = 2;
                role_id = 1;
                call_status = "confirmed";
                notes = "Called for full session.";
              }
              {
                id = 2;
                rehearsal_id = 1;
                user_id = 3;
                role_id = 2;
                call_status = "confirmed";
                notes = "Music director call.";
              }
            ];
            rehearsal_requirement = [
              {
                id = 1;
                production_id = 1;
                name = "Act I finale";
                category = "music";
                required_sessions = 2;
                status = "scheduled";
              }
            ];
            rehearsal_coverage = [
              {
                id = 1;
                requirement_id = 1;
                rehearsal_id = 1;
                status = "planned";
                notes = "First coverage pass.";
              }
            ];
            production_material = [
              {
                id = 1;
                production_id = 1;
                name = "Current vocal score";
                kind = "score";
                visibility = "participants";
                status = "current";
                description = "Authoritative vocal score for the Summer 2026 production.";
              }
            ];
            material_version = [
              {
                id = 1;
                material_id = 1;
                version_label = "v1";
                storage_uri = "duchyopera://materials/magic-flute/vocal-score-v1.pdf";
                checksum = "fixture";
                status = "current";
                notes = "Initial rehearsal score.";
                created_by_user_id = 1;
              }
            ];
            performance = [
              {
                id = 1;
                production_id = 1;
                venue_id = 1;
                starts_at = "2026-06-27T19:30:00Z";
                ends_at = "2026-06-27T22:00:00Z";
                price_pence = 2500;
                description = "Mozart's final opera staged for a Cornish charity audience.";
                status = "scheduled";
              }
              {
                id = 2;
                production_id = 1;
                venue_id = 1;
                starts_at = "2026-06-28T19:30:00Z";
                ends_at = "2026-06-28T21:30:00Z";
                price_pence = 3000;
                description = "Fixture used to prove unavailable catalog items stay out of checkout.";
                status = "cancelled";
              }
            ];
          };
        };
      })
      ../../../../nix/model/modules/identity.nix
      ../../../../nix/model/modules/user.nix
      ../../../../nix/model/modules/user_b2c.nix # Adds customer_type to user
      ../../../../nix/model/modules/api_key.nix
      ../../../../nix/model/modules/artist_profile.nix
      ../../../../nix/model/modules/performance.nix
      ../../../../nix/model/modules/ticket.nix
      ../../../../nix/model/modules/transaction.nix
      ../../../../nix/model/modules/booking.nix
      ../../../../nix/model/modules/venue.nix
      ../../../../nix/model/bundles/production-scheduling.nix
      ../../../../nix/model/bundles/materials-library.nix
    ];
  };
in {
  organization = modules.config.organization;
  auth = modules.config.auth;
  audit = modules.config.audit;
  seed = modules.config.seed;
  integrations = modules.config.integrations;
  effectContracts = modules.config.effectContracts;
  presentation = modules.config.presentation;
  tables = modules.config.tables;
  derived = modules.config.derived;
  refs = modules.config.refs;
  entityRefs = modules.config.entityRefs;
  relationships = modules.config.relationships;
  constraints = modules.config.constraints;
  operations = composeLib.processOperations modules.config.tables modules.config.relationships modules.config.operations;
}
