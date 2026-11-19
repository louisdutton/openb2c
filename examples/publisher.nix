{...}: {
  name = "Digital publisher";
  description = "Digital books and reader membership paid by bank transfer";
  currency = "GBP";

  accounting.enable = true;
  billing.enable = true;
  documents.enable = true;
  purchasing.enable = true;
  banking.enable = true;
  tax.enable = true;

  customers.enable = true;
  catalog.enable = true;
  orders.enable = true;
  payments.enable = true;
  subscriptions.enable = true;
  entitlements.enable = true;
  downloads.enable = true;

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
