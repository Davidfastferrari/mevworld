use alloy::sol_types::sol;
use alloy::primitives::{Address, U256, I24, U160, U24, U112, U128, U16, U8, U32};

// define our flash swap contract
sol!(
    #[sol(rpc)]
    FlashSwap,
    "./abi/FlashSwap.json"
);

sol!(
    #[sol(rpc)]
    FlashQuoter,
    "./abi/FlashQuoter.json"
);

pub use FlashSwap::FlashSwapInstance;
pub use FlashQuoter::FlashQuoterInstance;

// Abi Generation for an ERC20 token
sol!(
    #[sol(rpc)]
    contract ERC20Token {
        function approve(address spender, uint256 amount) external returns (bool success);
        function balanceOf(address account) external view returns (uint256);
    }
);

// State function signatures
sol! {
    contract V2State {
        function getReserves() external view returns (uint112 reserve0, uint112 reserve1, uint32 blockTimestampLast);
    }
}

sol! {
    contract V3State {
        function liquidity() external view returns (uint128);
        function slot0() external view returns (
            uint160 sqrtPriceX96,
            int24 tick,
            uint16 observationIndex,
            uint16 observationCardinality,
            uint16 observationCardinalityNext,
            uint8 feeProtocol,
            bool unlocked
        );
    }
}

// Swap function signatures
sol!(
    #[sol(rpc)]
    contract V2Swap {
        function swapExactTokensForTokens(
            uint256 amountIn,
            uint256 amountOutMin,
            address[] calldata path,
            address to,
            uint256 deadline
        ) external returns (uint256[] memory amounts);
    }
);

sol!(
    #[sol(rpc)]
    contract V3Swap {
        struct ExactInputSingleParams {
            address tokenIn;
            address tokenOut;
            uint24 fee;
            address recipient;
            uint256 amountIn;
            uint256 amountOutMinimum;
            uint160 sqrtPriceLimitX96;
        }
        function exactInputSingle(ExactInputSingleParams calldata params) external payable returns (uint256 amountOut);
    }
);

sol!(
    #[sol(rpc)]
    contract V3SwapDeadline {
        struct ExactInputSingleParams {
            address tokenIn;
            address tokenOut;
            uint24 fee;
            address recipient;
            uint256 deadline;
            uint256 amountIn;
            uint256 amountOutMinimum;
            uint160 sqrtPriceLimitX96;
        }
        function exactInputSingle(ExactInputSingleParams calldata params) external payable returns (uint256 amountOut);
    }
);

sol!(
    #[sol(rpc)]
    interface V2Aerodrome {
        struct Route {
            address from;
            address to;
            bool stable;
            address factory;
        }
        function swapExactTokensForTokens(
            uint256 amountIn,
            uint256 amountOutMin,
            Route[] calldata routes,
            address to,
            uint256 deadline
        ) external returns (uint256[] memory amounts);
    }
);

sol!(
    #[sol(rpc)]
    contract V3SwapDeadlineTick {
        struct ExactInputSingleParams {
            address tokenIn;
            address tokenOut;
            int24 tickSpacing;
            address recipient;
            uint256 deadline;
            uint256 amountIn;
            uint256 amountOutMinimum;
            uint160 sqrtPriceLimitX96;
        }
        function exactInputSingle(ExactInputSingleParams calldata params) external payable returns (uint256 amountOut);
    }
);
