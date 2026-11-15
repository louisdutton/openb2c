{...}: {
  name = "Digital publisher";
  description = "Digital books and reader membership; simulated payments only";
  currency = "GBP";

  customers.enable = true;
  auth.password.enable = true;
  catalog.enable = true;
  orders.enable = true;
  payments.enable = true;
  subscriptions.enable = true;
  entitlements.enable = true;

  demo.products = {
    digital-book = {
      name = "Digital book";
      prices = [{amount = 1500;}];
    };
    reader-membership = {
      name = "Reader membership";
      prices = [
        {
          amount = 500;
          interval = "month";
        }
        {
          amount = 5000;
          interval = "year";
        }
      ];
    };
  };
}
