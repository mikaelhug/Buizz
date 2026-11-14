use buizz_core::market::FundamentalMetric;
use buizz_core::market::FundamentalMetric as M;

use super::UnitKind;
use super::UnitKind::{Currency, CurrencyPerShare, Shares};

pub const US_GAAP: &str = "us-gaap";
pub const IFRS: &str = "ifrs-full";
pub const DEI: &str = "dei";
pub const COVER_PAGE_SHARES: &str = "EntityCommonStockSharesOutstanding";

type Entry = (&'static str, FundamentalMetric, UnitKind);

pub const US_GAAP_PRIMARY: &[Entry] = &[
    ("Revenues", M::Revenue, Currency),
    (
        "RevenueFromContractWithCustomerExcludingAssessedTax",
        M::Revenue,
        Currency,
    ),
    ("SalesRevenueNet", M::Revenue, Currency),
    (
        "RevenueFromContractWithCustomerIncludingAssessedTax",
        M::Revenue,
        Currency,
    ),
    ("CostOfRevenue", M::CostOfRevenue, Currency),
    ("CostOfGoodsAndServicesSold", M::CostOfRevenue, Currency),
    ("CostOfGoodsSold", M::CostOfRevenue, Currency),
    ("GrossProfit", M::GrossProfit, Currency),
    (
        "ResearchAndDevelopmentExpense",
        M::ResearchDevelopment,
        Currency,
    ),
    (
        "SellingGeneralAndAdministrativeExpense",
        M::SgaExpense,
        Currency,
    ),
    ("OperatingExpenses", M::OperatingExpenses, Currency),
    ("OperatingIncomeLoss", M::OperatingIncome, Currency),
    ("InterestExpense", M::InterestExpense, Currency),
    ("InterestExpenseNonoperating", M::InterestExpense, Currency),
    (
        "IncomeLossFromContinuingOperationsBeforeIncomeTaxesExtraordinaryItemsNoncontrollingInterest",
        M::PretaxIncome,
        Currency,
    ),
    (
        "IncomeLossFromContinuingOperationsBeforeIncomeTaxesMinorityInterestAndIncomeLossFromEquityMethodInvestments",
        M::PretaxIncome,
        Currency,
    ),
    ("IncomeTaxExpenseBenefit", M::IncomeTax, Currency),
    ("NetIncomeLoss", M::NetIncome, Currency),
    ("ProfitLoss", M::NetIncome, Currency),
    ("EarningsPerShareDiluted", M::EpsDiluted, CurrencyPerShare),
    (
        "EarningsPerShareBasicAndDiluted",
        M::EpsDiluted,
        CurrencyPerShare,
    ),
    ("Assets", M::TotalAssets, Currency),
    ("AssetsCurrent", M::CurrentAssets, Currency),
    (
        "CashAndCashEquivalentsAtCarryingValue",
        M::CashAndEquivalents,
        Currency,
    ),
    ("InventoryNet", M::Inventory, Currency),
    ("AccountsReceivableNetCurrent", M::Receivables, Currency),
    ("PropertyPlantAndEquipmentNet", M::PpeNet, Currency),
    (
        "PropertyPlantAndEquipmentAndFinanceLeaseRightOfUseAssetAfterAccumulatedDepreciationAndAmortization",
        M::PpeNet,
        Currency,
    ),
    ("Goodwill", M::Goodwill, Currency),
    ("LiabilitiesCurrent", M::CurrentLiabilities, Currency),
    ("LongTermDebtNoncurrent", M::LongTermDebt, Currency),
    ("LongTermDebt", M::LongTermDebt, Currency),
    (
        "LongTermDebtAndCapitalLeaseObligations",
        M::LongTermDebt,
        Currency,
    ),
    ("Liabilities", M::TotalLiabilities, Currency),
    ("StockholdersEquity", M::Equity, Currency),
    (
        "StockholdersEquityIncludingPortionAttributableToNoncontrollingInterest",
        M::Equity,
        Currency,
    ),
    (
        "StockholdersEquityIncludingPortionAttributableToNoncontrollingInterest",
        M::TotalEquity,
        Currency,
    ),
    (
        "NetCashProvidedByUsedInOperatingActivities",
        M::OperatingCashFlow,
        Currency,
    ),
    (
        "NetCashProvidedByUsedInOperatingActivitiesContinuingOperations",
        M::OperatingCashFlow,
        Currency,
    ),
    (
        "DepreciationDepletionAndAmortization",
        M::DepreciationAmortization,
        Currency,
    ),
    (
        "DepreciationAndAmortization",
        M::DepreciationAmortization,
        Currency,
    ),
    (
        "DepreciationAmortizationAndAccretionNet",
        M::DepreciationAmortization,
        Currency,
    ),
    ("ShareBasedCompensation", M::StockBasedComp, Currency),
    (
        "AllocatedShareBasedCompensationExpense",
        M::StockBasedComp,
        Currency,
    ),
    (
        "PaymentsToAcquirePropertyPlantAndEquipment",
        M::Capex,
        Currency,
    ),
    ("PaymentsToAcquireProductiveAssets", M::Capex, Currency),
    (
        "NetCashProvidedByUsedInInvestingActivities",
        M::InvestingCashFlow,
        Currency,
    ),
    (
        "NetCashProvidedByUsedInInvestingActivitiesContinuingOperations",
        M::InvestingCashFlow,
        Currency,
    ),
    (
        "NetCashProvidedByUsedInFinancingActivities",
        M::FinancingCashFlow,
        Currency,
    ),
    (
        "NetCashProvidedByUsedInFinancingActivitiesContinuingOperations",
        M::FinancingCashFlow,
        Currency,
    ),
    ("PaymentsOfDividendsCommonStock", M::DividendsPaid, Currency),
    ("PaymentsOfDividends", M::DividendsPaid, Currency),
    (
        "PaymentsForRepurchaseOfCommonStock",
        M::ShareRepurchases,
        Currency,
    ),
    (
        "PaymentsToAcquireBusinessesNetOfCashAcquired",
        M::Acquisitions,
        Currency,
    ),
    (
        "PaymentsToAcquireBusinessesAndInterestInAffiliatesNetOfCashAcquired",
        M::Acquisitions,
        Currency,
    ),
    ("CommonStockSharesOutstanding", M::SharesOutstanding, Shares),
    ("DebtCurrent", M::ShortTermDebt, Currency),
    ("ShortTermBorrowings", M::ShortTermBorrowings, Currency),
    ("LongTermDebtCurrent", M::CurrentLongTermDebt, Currency),
    (
        "LongTermDebtNoncurrent",
        M::LongTermDebtNoncurrent,
        Currency,
    ),
    (
        "LongTermDebtAndCapitalLeaseObligations",
        M::LongTermDebtNoncurrent,
        Currency,
    ),
    ("PreferredStockValue", M::PreferredStock, Currency),
    (
        "PreferredStockValueOutstanding",
        M::PreferredStock,
        Currency,
    ),
    ("MinorityInterest", M::MinorityInterest, Currency),
    (
        "RetainedEarningsAccumulatedDeficit",
        M::RetainedEarnings,
        Currency,
    ),
    ("AccruedIncomeTaxesCurrent", M::IncomeTaxesPayable, Currency),
    ("ShortTermInvestments", M::ShortTermInvestments, Currency),
    (
        "MarketableSecuritiesCurrent",
        M::ShortTermInvestments,
        Currency,
    ),
    (
        "AvailableForSaleSecuritiesDebtSecuritiesCurrent",
        M::ShortTermInvestments,
        Currency,
    ),
    (
        "AvailableForSaleSecuritiesCurrent",
        M::ShortTermInvestments,
        Currency,
    ),
    ("ConvertibleDebt", M::ConvertibleDebt, Currency),
    ("ConvertibleNotesPayable", M::ConvertibleDebt, Currency),
    ("SecuredDebt", M::SecuredDebt, Currency),
    ("Land", M::Land, Currency),
    ("LandAndLandImprovements", M::Land, Currency),
    ("BuildingsAndImprovementsGross", M::Buildings, Currency),
    ("PropertyPlantAndEquipmentGross", M::PpeGross, Currency),
    (
        "PropertyPlantAndEquipmentAndFinanceLeaseRightOfUseAssetBeforeAccumulatedDepreciationAndAmortization",
        M::PpeGross,
        Currency,
    ),
    ("AdvertisingExpense", M::Advertising, Currency),
];

pub const IFRS_PRIMARY: &[Entry] = &[
    ("Revenue", M::Revenue, Currency),
    ("RevenueFromContractsWithCustomers", M::Revenue, Currency),
    ("CostOfSales", M::CostOfRevenue, Currency),
    ("GrossProfit", M::GrossProfit, Currency),
    (
        "ProfitLossFromOperatingActivities",
        M::OperatingIncome,
        Currency,
    ),
    ("ProfitLossBeforeTax", M::PretaxIncome, Currency),
    (
        "IncomeTaxExpenseContinuingOperations",
        M::IncomeTax,
        Currency,
    ),
    (
        "ResearchAndDevelopmentExpense",
        M::ResearchDevelopment,
        Currency,
    ),
    (
        "ProfitLossAttributableToOwnersOfParent",
        M::NetIncome,
        Currency,
    ),
    ("ProfitLoss", M::NetIncome, Currency),
    (
        "DilutedEarningsLossPerShare",
        M::EpsDiluted,
        CurrencyPerShare,
    ),
    (
        "BasicAndDilutedEarningsLossPerShare",
        M::EpsDiluted,
        CurrencyPerShare,
    ),
    ("Assets", M::TotalAssets, Currency),
    ("Liabilities", M::TotalLiabilities, Currency),
    ("CurrentAssets", M::CurrentAssets, Currency),
    ("CurrentLiabilities", M::CurrentLiabilities, Currency),
    ("EquityAttributableToOwnersOfParent", M::Equity, Currency),
    ("Equity", M::Equity, Currency),
    ("Equity", M::TotalEquity, Currency),
    ("CashAndCashEquivalents", M::CashAndEquivalents, Currency),
    ("Goodwill", M::Goodwill, Currency),
    ("Inventories", M::Inventory, Currency),
    ("TradeAndOtherCurrentReceivables", M::Receivables, Currency),
    ("PropertyPlantAndEquipment", M::PpeNet, Currency),
    ("LongtermBorrowings", M::LongTermDebt, Currency),
    (
        "NoncurrentPortionOfNoncurrentBorrowings",
        M::LongTermDebt,
        Currency,
    ),
    (
        "CashFlowsFromUsedInOperatingActivities",
        M::OperatingCashFlow,
        Currency,
    ),
    (
        "CashFlowsFromUsedInInvestingActivities",
        M::InvestingCashFlow,
        Currency,
    ),
    (
        "CashFlowsFromUsedInFinancingActivities",
        M::FinancingCashFlow,
        Currency,
    ),
    (
        "PurchaseOfPropertyPlantAndEquipmentClassifiedAsInvestingActivities",
        M::Capex,
        Currency,
    ),
    (
        "PaymentsToAcquirePropertyPlantAndEquipment",
        M::Capex,
        Currency,
    ),
    (
        "DividendsPaidClassifiedAsFinancingActivities",
        M::DividendsPaid,
        Currency,
    ),
    (
        "DepreciationAndAmortisationExpense",
        M::DepreciationAmortization,
        Currency,
    ),
    (
        "DepreciationAmortisationAndImpairmentLossReversalOfImpairmentLossRecognisedInProfitOrLoss",
        M::DepreciationAmortization,
        Currency,
    ),
    ("SharebasedPaymentExpense", M::StockBasedComp, Currency),
    (
        "WeightedAverageNumberOfOrdinarySharesOutstanding",
        M::SharesOutstanding,
        Shares,
    ),
    (
        "AdjustedWeightedAverageShares",
        M::SharesOutstanding,
        Shares,
    ),
    ("NumberOfSharesOutstanding", M::SharesOutstanding, Shares),
    (
        "CurrentBorrowingsAndCurrentPortionOfNoncurrentBorrowings",
        M::ShortTermDebt,
        Currency,
    ),
    ("ShorttermBorrowings", M::ShortTermBorrowings, Currency),
    (
        "CurrentPortionOfLongtermBorrowings",
        M::CurrentLongTermDebt,
        Currency,
    ),
    (
        "NoncurrentPortionOfNoncurrentBorrowings",
        M::LongTermDebtNoncurrent,
        Currency,
    ),
    ("NoncontrollingInterests", M::MinorityInterest, Currency),
    ("RetainedEarnings", M::RetainedEarnings, Currency),
    (
        "CurrentTaxLiabilitiesCurrent",
        M::IncomeTaxesPayable,
        Currency,
    ),
    ("CurrentTaxLiabilities", M::IncomeTaxesPayable, Currency),
];

pub const WEIGHTED_AVERAGE_SHARES: &[&str] = &[
    "WeightedAverageNumberOfSharesOutstandingBasic",
    "WeightedAverageNumberOfShareOutstandingBasicAndDiluted",
];
